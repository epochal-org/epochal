"""Configuration, errors and retries, the same for every call."""

from __future__ import annotations

import datetime as dt
from collections.abc import Callable

import httpx
import pytest
from epochal.api import (
    APIError,
    AsyncClient,
    Client,
    InvalidArgumentError,
    MissingApiKeyError,
    TransportError,
)
from fake_api import Api, Fake, error

AS_OF = "2026-03-01T00:00:00Z"
SEARCH = {"as_of": AS_OF, "request_id": "r", "results": []}
SANDBOX = {
    "id": "sbx_1",
    "state": "live",
    "as_of": AS_OF,
    "network": "none",
    "vcpus": 2,
    "memory_gib": 4,
    "timeout": "1h",
    "idle_timeout": "10m",
    "created_at": AS_OF,
    "metadata": {},
    "apps": [],
}
MakeApi = Callable[..., Api]


def ok(_: httpx.Request) -> httpx.Response:
    return httpx.Response(200, json=SEARCH)


def test_a_wrong_key_is_refused_with_the_apis_error(make_api: MakeApi) -> None:
    with pytest.raises(APIError) as caught:
        make_api(Fake(ok), api_key="bad-key").search("q")

    assert caught.value.status == 401
    assert caught.value.kind == "invalid_api_key"
    assert caught.value.message == "No key matches that token."
    assert str(caught.value) == "No key matches that token. (HTTP 401)"


def sandbox(_: httpx.Request) -> httpx.Response:
    return httpx.Response(200, json=SANDBOX)


def test_a_proxy_failure_keeps_its_status_and_is_retried_until_it_passes(
    make_api: MakeApi,
) -> None:
    fake = Fake(sandbox, queue=[httpx.Response(502, text="<html>bad gateway</html>")] * 2)

    found = make_api(fake).get_sandbox("sbx_1")

    assert found.id == "sbx_1"
    assert len(fake.seen) == 3
    assert len(fake.slept) == 2


def test_a_dropped_connection_is_retried(make_api: MakeApi) -> None:
    fake = Fake(sandbox, queue=[httpx.ConnectError("reset")])

    make_api(fake).get_sandbox("sbx_1")

    assert len(fake.seen) == 2


@pytest.mark.parametrize("status", [500, 502, 0])
def test_a_billed_call_the_server_may_have_done_is_not_repeated(
    make_api: MakeApi, status: int
) -> None:
    fake = Fake(ok, queue=[httpx.Response(status) if status else httpx.ConnectError("reset")])

    with pytest.raises((APIError, TransportError)):
        make_api(fake).search("q")

    assert len(fake.seen) == 1, "search, fetch and page carry no idempotency key"


def test_retries_stop_after_max_retries_with_the_last_error(make_api: MakeApi) -> None:
    fake = Fake(lambda _: httpx.Response(503))

    with pytest.raises(APIError) as caught:
        make_api(fake, max_retries=2).search("q")

    assert caught.value.status == 503
    assert caught.value.kind == "unknown"
    assert caught.value.retryable
    assert len(fake.seen) == 3


def test_max_retries_zero_sends_once(make_api: MakeApi) -> None:
    fake = Fake(ok, queue=[httpx.ReadTimeout("slow")])

    with pytest.raises(TransportError) as caught:
        make_api(fake, max_retries=0).search("q")

    assert isinstance(caught.value.__cause__, httpx.ReadTimeout)
    assert len(fake.seen) == 1


def test_a_rate_limit_waits_as_long_as_the_api_says(make_api: MakeApi) -> None:
    limited = error(429, "rate_limit_exceeded", "Over the organization's rate limit.")
    limited.headers["retry-after"] = "7"
    fake = Fake(ok, queue=[limited])

    make_api(fake).search("q")

    assert fake.slept == [7.0]


def test_a_retry_after_past_the_budget_ends_the_call_with_its_error(make_api: MakeApi) -> None:
    limited = error(429, "rate_limit_exceeded", "Slow down.")
    limited.headers["retry-after"] = "3600"
    fake = Fake(ok, queue=[limited])

    with pytest.raises(APIError) as caught:
        make_api(fake).search("q")

    assert caught.value.retry_after == dt.timedelta(hours=1)
    assert len(fake.seen) == 1


@pytest.mark.parametrize("value", ["inf", "1e20", "172800"])
def test_a_retry_after_too_long_for_the_budget_ends_the_call(make_api: MakeApi, value: str) -> None:
    limited = error(429, "rate_limit_exceeded", "Slow down.")
    limited.headers["retry-after"] = value
    fake = Fake(ok, queue=[limited])

    with pytest.raises(APIError):
        make_api(fake).search("q")

    assert (len(fake.seen), fake.slept) == (1, [])


@pytest.mark.parametrize("value", ["-1", "nan", "Wed, 21 Oct 2026 07:28:00 GMT"])
def test_a_retry_after_that_is_no_usable_delay_falls_back_to_backoff(
    make_api: MakeApi, value: str
) -> None:
    limited = error(429, "rate_limit_exceeded", "Slow down.")
    limited.headers["retry-after"] = value
    fake = Fake(ok, queue=[limited])

    make_api(fake).search("q")

    assert len(fake.slept) == 1
    assert 0.25 <= fake.slept[0] <= 0.5


def test_a_long_poll_may_outlast_a_short_client_timeout(make_api: MakeApi) -> None:
    seen: list[dict[str, float | None]] = []

    def box(request: httpx.Request) -> httpx.Response:
        seen.append(request.extensions["timeout"])
        return httpx.Response(200, json=SANDBOX)

    api = make_api(Fake(box), timeout=2)
    api.get_sandbox("sbx_1")
    api.get_sandbox("sbx_1", wait=dt.timedelta(seconds=30))

    assert seen[0]["read"] == 2
    assert seen[1]["read"] is not None
    assert seen[1]["read"] > 30
    assert seen[1]["connect"] == 2


def test_a_refusal_is_raised_at_once(make_api: MakeApi) -> None:
    fake = Fake(lambda _: error(400, "invalid_request", "Bad query."))

    with pytest.raises(APIError):
        make_api(fake).search("q")

    assert len(fake.seen) == 1


def test_a_redirect_is_never_followed_so_the_key_never_leaves(make_api: MakeApi) -> None:
    fake = Fake(lambda _: httpx.Response(307, headers={"location": "https://evil.test/steal"}))

    with pytest.raises(APIError) as caught:
        make_api(fake).search("q")

    assert caught.value.status == 307
    assert {r.url.host for r in fake.seen} == {"api.test"}


def test_a_redirect_is_not_followed_even_by_a_client_that_follows_them(
    make_api: MakeApi,
) -> None:
    fake = Fake(lambda _: httpx.Response(307, headers={"location": "https://evil.test/steal"}))
    api = make_api(fake, follow_redirects=True)

    with pytest.raises(APIError):
        api.search("q")

    assert {r.url.host for r in fake.seen} == {"api.test"}


def test_a_naive_instant_is_refused_before_any_request(make_api: MakeApi) -> None:
    fake = Fake(ok)

    with pytest.raises(InvalidArgumentError):
        make_api(fake, as_of=dt.datetime(2026, 3, 1))

    assert fake.seen == []


def test_the_portal_holds_new_work_to_its_instant_and_reaches_a_box_by_id_alone(
    make_api: MakeApi,
) -> None:
    fake = Fake(lambda _: httpx.Response(200, json=SANDBOX))
    api = make_api(fake, as_of="2026-03-01")

    api.create_sandbox()
    api.get_sandbox("sbx_1")

    assert api.portal.as_of == dt.datetime(2026, 3, 1, 23, 59, 59, tzinfo=dt.UTC)
    assert fake.seen[0].headers["accept-datetime"] == "2026-03-01T23:59:59Z"
    assert "accept-datetime" not in fake.seen[1].headers


def test_the_portals_instant_cannot_be_changed_after_it_is_made(make_api: MakeApi) -> None:
    fake = Fake(lambda _: httpx.Response(200, json=SEARCH))
    api = make_api(fake, as_of="2026-03-01")

    with pytest.raises(AttributeError):
        api.portal.as_of = dt.datetime(2024, 1, 1, tzinfo=dt.UTC)
    api.search("q")

    assert fake.last.headers["accept-datetime"] == "2026-03-01T23:59:59Z"


@pytest.mark.parametrize("flavour", [Client, AsyncClient])
def test_the_key_and_url_come_from_the_environment_unless_given(
    monkeypatch: pytest.MonkeyPatch, flavour: type[Client] | type[AsyncClient]
) -> None:
    monkeypatch.delenv("EPOCHAL_API_KEY", raising=False)
    monkeypatch.delenv("EPOCHAL_BASE_URL", raising=False)
    with pytest.raises(MissingApiKeyError):
        flavour()
    monkeypatch.setenv("EPOCHAL_API_KEY", "")
    with pytest.raises(MissingApiKeyError):
        flavour()

    monkeypatch.setenv("EPOCHAL_API_KEY", "env-key")
    assert flavour().base_url == "https://api.epochal.com"
    monkeypatch.setenv("EPOCHAL_BASE_URL", "http://localhost:8080/")
    assert flavour().base_url == "http://localhost:8080"
    assert flavour(base_url="https://set.test").base_url == "https://set.test"
    with pytest.raises(InvalidArgumentError):
        flavour(base_url="api.epochal.com")


def test_repr_never_shows_the_key() -> None:
    client = Client(api_key="sk-secret-123")
    try:
        assert "sk-secret-123" not in repr(client)
        assert "https://api.epochal.com" in repr(client)
    finally:
        client.close()
