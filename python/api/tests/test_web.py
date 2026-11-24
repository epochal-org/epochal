from __future__ import annotations

import datetime as dt
from collections.abc import Callable

import httpx
import pytest
from epochal.api import APIError, DecodeError
from epochal.api.entities import OrganizationProperties
from epochal.api.time_portal import Category, PageError, PageStatus
from fake_api import Api, Fake, body, error

AS_OF = "2026-03-01T00:00:00Z"
TOKYO = dt.timezone(dt.timedelta(hours=9))

SEARCH = {
    "as_of": AS_OF,
    "request_id": "req_9Zp4kL",
    "results": [
        {
            "title": "Battery plant start pushed back to late 2027",
            "url": "https://news.example.com/energy/battery-plant-delay",
            "snippet": "The cell maker said commissioning would slip by three quarters…",
            "published_at": "2026-02-11T06:12:00Z",
            "domain": "example.com",
            "language": "en",
            "entity": None,
        }
    ],
    "entities": [],
}
NVIDIA = {
    "id": "https://world.epochal.com/organization/nvidia",
    "type": "organization",
    "schema_version": 1,
    "updated_at": "2026-02-28T00:00:00Z",
    "properties": {"name": "NVIDIA"},
}
ENTITY_SEARCH = {
    "as_of": AS_OF,
    "request_id": "req_9Zp4kL",
    "results": [
        {
            "title": "NVIDIA",
            "url": "https://world.epochal.com/organization/nvidia",
            "snippet": "NVIDIA designs GPUs.",
            "published_at": None,
            "domain": "world.epochal.com",
            "language": "en",
            "entity": NVIDIA,
        },
        {
            "title": "NVIDIA earnings beat",
            "url": "https://news.example.com/nvidia-earnings",
            "snippet": "Revenue rose…",
            "published_at": "2026-02-26T21:00:00Z",
            "domain": "example.com",
            "language": "en",
            "entity": None,
        },
    ],
    "entities": [],
}
OLDER_SERVER_SEARCH = {
    "as_of": AS_OF,
    "request_id": "req_9Zp4kL",
    "results": [
        {
            "title": "NVIDIA earnings beat",
            "url": "https://news.example.com/nvidia-earnings",
            "snippet": "Revenue rose…",
        }
    ],
}
FETCH = {
    "as_of": AS_OF,
    "request_id": "req_9Zp4kL",
    "status": "success",
    "id": "6b81feebde1a4821a125fb307894e7ad",
    "url": "https://news.example.com/energy/battery-plant-delay",
    "final_url": "https://news.example.com/energy/battery-plant-delay/",
    "title": "Battery plant start pushed back to late 2027",
    "description": "The cell maker said commissioning would slip.",
    "author": "Jane Doe",
    "published_at": "2026-02-11T06:12:00Z",
    "language": "en",
    "content": "# Battery plant start pushed back to late 2027\n\nThe cell maker said…",
    "truncated": True,
    "entity": None,
}
UNREAD = {
    "as_of": AS_OF,
    "request_id": "req_9Zp4kL",
    "status": "error",
    "page_error": {"http_status_code": 404, "message": "Page not found."},
    "url": "https://news.example.com/energy/missing",
    "final_url": "https://news.example.com/energy/missing",
    "title": "",
    "content": "",
    "truncated": False,
    "entity": None,
}
PAGE_HTML = b'<html><head><title>Battery plant</title></head><body>{"not": "json"}</body></html>'


def page_status(code: str, message: str) -> dict[str, str]:
    return {"x-fetch-status-code": code, "x-fetch-status-message": message}


def route(request: httpx.Request) -> httpx.Response:
    sent = body(request)
    if request.url.path == "/v1/time-portal/search":
        if sent["query"] == "nvidia":
            return httpx.Response(200, json=ENTITY_SEARCH)
        if sent["query"] == "older server":
            return httpx.Response(200, json=OLDER_SERVER_SEARCH)
        if sent["query"] == "garbled":
            return httpx.Response(200, json={"results": "not a list"})
        return httpx.Response(200, json=SEARCH)
    if request.url.path == "/v1/time-portal/fetch":
        if sent.get("format") == "page":
            if sent["url"].endswith("/missing"):
                return httpx.Response(200, headers=page_status("404", "Page not found."))
            if sent["url"].endswith("/never"):
                return httpx.Response(
                    200, headers=page_status("0", "No copy at or before Accept-Datetime.")
                )
            headers = {
                "content-type": "text/html; charset=utf-8",
                "memento-datetime": AS_OF,
                **page_status("200", "OK."),
            }
            return httpx.Response(200, headers=headers, content=PAGE_HTML)
        if sent["url"].endswith("/missing"):
            return httpx.Response(200, json=UNREAD)
        if "x.example" in sent["url"]:
            return error(403, "url_not_allowed", "This URL can't be fetched.")
        return httpx.Response(200, json=FETCH)
    return httpx.Response(404)


MakeApi = Callable[..., Api]


def test_search_sends_the_instant_and_only_the_fields_set(make_api: MakeApi) -> None:
    fake = Fake(route)
    found = make_api(fake).search("european battery gigafactory delays", num_results=5)

    seen = fake.last
    assert str(seen.url) == "https://api.test/v1/time-portal/search"
    assert seen.headers["x-api-key"] == "good-key"
    assert seen.headers["accept-datetime"] == AS_OF
    assert seen.headers["user-agent"].startswith("epochal-api-python/")
    assert body(seen) == {"query": "european battery gigafactory delays", "num_results": 5}
    hit = found.results[0]
    assert hit.url == "https://news.example.com/energy/battery-plant-delay"
    assert hit.published_at == dt.datetime(2026, 2, 11, 6, 12, tzinfo=dt.UTC)
    assert hit.text is None
    assert hit.entity is None
    assert found.request_id == "req_9Zp4kL"
    assert found.entities == []


def test_a_day_is_published_after_from_its_first_second_but_as_of_to_its_last(
    make_api: MakeApi,
) -> None:
    fake = Fake(route)
    make_api(fake, as_of="2026-03-01").search("q", published_after=dt.date(2026, 2, 1))

    assert body(fake.last)["published_after"] == "2026-02-01T00:00:00Z"
    assert fake.last.headers["accept-datetime"] == "2026-03-01T23:59:59Z"


def test_category_lists_only_the_names_the_api_takes() -> None:
    assert [c.value for c in Category] == [
        "news",
        "finance",
        "legal",
        "publications",
        "companies",
        "people",
        "maps",
    ]


def test_categories_and_extra_fields_go_out_as_given(make_api: MakeApi) -> None:
    fake = Fake(route)
    make_api(fake).search("q", categories=[Category.NEWS, "patents"], extra_body={"rerank": True})

    assert body(fake.last) == {"query": "q", "categories": ["news", "patents"], "rerank": True}


def test_a_search_naming_an_entity_returns_it_on_its_world_page_result(make_api: MakeApi) -> None:
    found = make_api(Fake(route)).search("nvidia")

    [page, news] = found.results
    assert page.entity is not None
    assert page.entity.id == page.url
    assert page.entity.type == "organization"
    assert page.entity.properties == {"name": "NVIDIA"}
    assert isinstance(page.entity.typed, OrganizationProperties)
    assert page.entity.typed.name == "NVIDIA"
    assert news.entity is None
    assert found.entities == []


def test_a_response_from_an_older_server_reads_without_entities(make_api: MakeApi) -> None:
    found = make_api(Fake(route)).search("older server")

    assert found.results[0].entity is None
    assert found.entities == []


def test_fetch_reads_the_page_and_its_dates(make_api: MakeApi) -> None:
    fake = Fake(route)
    url = "https://news.example.com/energy/battery-plant-delay"
    page = make_api(fake).fetch(url, max_characters=2000)

    assert body(fake.last) == {"url": url, "max_characters": 2000}
    assert page.final_url == url + "/"
    assert page.author == "Jane Doe"
    assert page.status is PageStatus.SUCCESS
    assert page.page_error is None
    assert page.truncated
    assert page.content.startswith("# Battery plant")


def test_a_page_that_could_not_be_read_is_an_answer_with_the_sites_status(
    make_api: MakeApi,
) -> None:
    page = make_api(Fake(route)).fetch("https://news.example.com/energy/missing")

    assert page.status is PageStatus.ERROR
    assert page.page_error is not None
    assert page.page_error.http_status_code == 404
    assert str(page.page_error) == "Page not found. (HTTP 404)"
    assert page.content == ""


def test_a_url_never_fetched_is_an_api_error_not_retried(make_api: MakeApi) -> None:
    fake = Fake(route)
    with pytest.raises(APIError) as caught:
        make_api(fake).fetch("https://x.example/home")

    assert (caught.value.status, caught.value.kind) == (403, "url_not_allowed")
    assert len(fake.seen) == 1
    assert caught.value.request_id == "req_9Zp4kL"
    assert not caught.value.retryable


def test_page_returns_the_document_itself(make_api: MakeApi) -> None:
    fake = Fake(route)
    url = "https://news.example.com/energy/battery-plant-delay"
    doc = make_api(fake).page(url)

    assert body(fake.last) == {"url": url, "format": "page"}
    assert doc.status is PageStatus.SUCCESS
    assert doc.page_error is None
    assert doc.body == PAGE_HTML
    assert doc.content_type == "text/html; charset=utf-8"
    assert doc.memento_datetime == AS_OF


@pytest.mark.parametrize(
    ("url", "code", "message"),
    [
        ("https://news.example.com/missing", 404, "Page not found."),
        ("https://news.example.com/never", None, "No copy at or before Accept-Datetime."),
    ],
)
def test_a_page_that_could_not_be_read_is_an_empty_document_with_its_status(
    make_api: MakeApi, url: str, code: int | None, message: str
) -> None:
    doc = make_api(Fake(route)).page(url)

    assert doc.status is PageStatus.ERROR
    assert doc.page_error == PageError(message, code)
    assert doc.body == b""
    assert doc.content_type is None


def test_a_success_off_the_documented_shape_is_a_decode_error(make_api: MakeApi) -> None:
    with pytest.raises(DecodeError):
        make_api(Fake(route)).search("garbled")


@pytest.mark.parametrize("body", ["null", "[]", '"text"'])
def test_a_success_whose_body_is_not_an_object_is_a_decode_error(
    make_api: MakeApi, body: str
) -> None:
    api = make_api(Fake(lambda _: httpx.Response(200, content=body)))

    with pytest.raises(DecodeError):
        api.fetch("https://example.com/")
    with pytest.raises(DecodeError):
        api.get_sandbox("sbx_1")


@pytest.mark.parametrize(
    ("as_of", "header"),
    [
        (
            dt.datetime(2026, 3, 1, 9, 30, 15, 999, tzinfo=dt.timezone(dt.timedelta(hours=9))),
            "2026-03-01T00:30:15Z",
        ),
        (dt.date(2026, 3, 1), "2026-03-01T23:59:59Z"),
        ("2026-03-01", "2026-03-01T23:59:59Z"),
        ("2026-03-01T12:00:00+02:00", "2026-03-01T10:00:00Z"),
    ],
)
def test_the_instant_goes_out_as_utc_in_whole_seconds(
    make_api: MakeApi, as_of: dt.datetime | dt.date | str, header: str
) -> None:
    fake = Fake(route)
    make_api(fake, as_of=as_of).search("q")

    assert fake.last.headers["accept-datetime"] == header
