"""``Entity.typed`` on the guide's example of each type, copied into ``spec/examples``."""

from __future__ import annotations

import datetime as dt
import json
from pathlib import Path
from typing import Any

import pytest
from epochal.api import DecodeError
from epochal.api.entities import (
    CryptoProperties,
    Currency,
    FundingRoundType,
    OrganizationProperties,
    PersonProperties,
    PlaceProperties,
    PlaceState,
    UnknownProperties,
    WeatherProperties,
    WeatherStatus,
    Weekday,
)
from epochal.api.time_portal import Entity, SearchResponse

EXAMPLES = Path(__file__).resolve().parents[3] / "spec/examples"


def example(kind: str) -> dict[str, Any]:
    sent: dict[str, Any] = json.loads((EXAMPLES / f"{kind}.json").read_text())
    return sent


def test_an_organization_reads_into_its_class() -> None:
    org = Entity.from_json(example("organization")).typed

    assert isinstance(org, OrganizationProperties)
    assert org.name == "Adobe"
    assert org.legal_name == "Adobe Systems Incorporated"
    assert org.tagline == "Empowering everyone to create. ✨"
    assert org.founded_year == 1982
    assert org.headquarters is not None
    assert org.headquarters.city == "San Jose"
    [round_] = org.funding_rounds
    assert round_.round_type is FundingRoundType.SERIES_UNKNOWN
    assert round_.announced_on == dt.date(1984, 11, 1)
    assert round_.currency is Currency.USD
    assert round_.investors[0].id is None
    assert round_.investors[0].name == "Apple"


def test_a_person_reads_into_its_class() -> None:
    person = Entity.from_json(example("person")).typed

    assert isinstance(person, PersonProperties)
    assert person.name == "Lisa Su"
    assert person.follower_count == 239177
    assert person.languages == []
    assert person.experience[0].title == "Chair & CEO"
    assert person.experience[0].start_date == "2022-02"
    assert person.experience[0].end_date is None


def test_a_place_reads_into_its_class() -> None:
    place = Entity.from_json(example("place")).typed

    assert isinstance(place, PlaceProperties)
    assert place.name == "Golden Gate Park"
    assert place.opening_hours[0].day is Weekday.MONDAY
    assert place.status.state is PlaceState.OPEN
    assert place.rating is not None
    assert place.rating.score == 4.8
    assert place.rating.distribution[4].count == 36276


def test_a_crypto_asset_reads_into_its_class() -> None:
    coin = Entity.from_json(example("crypto")).typed

    assert isinstance(coin, CryptoProperties)
    assert coin.symbol == "BTC"
    assert coin.stale_hours is None
    assert coin.latest is not None
    assert coin.latest.change_pct == -3.4976
    assert coin.quote is not None
    assert coin.quote.at == dt.datetime(2026, 11, 13, 14, 58, tzinfo=dt.UTC)
    assert coin.chart is not None
    assert len(coin.chart.prices) == 3
    assert coin.price_history is None


def test_weather_reads_into_its_class() -> None:
    weather = Entity.from_json(example("weather")).typed

    assert isinstance(weather, WeatherProperties)
    assert weather.timezone == "America/New_York"
    assert weather.current is not None
    assert weather.current.temperature.c == 8.3
    assert weather.yesterday is not None
    assert weather.yesterday.status is WeatherStatus.FINAL
    assert weather.forecast is not None
    assert weather.forecast.issued_at == dt.datetime(2026, 11, 20, 12, tzinfo=dt.UTC)
    assert weather.days is None


@pytest.mark.parametrize(
    "sent",
    [
        {
            "id": "https://world.epochal.com/index/spx",
            "type": "index",
            "schema_version": 1,
            "updated_at": "2026-11-20T21:00:00Z",
            "properties": {"name": "S&P 500", "level": 6012.5},
        },
        example("stock"),
    ],
    ids=["index", "stock"],
)
def test_a_type_this_version_does_not_model_keeps_its_properties(sent: dict[str, Any]) -> None:
    assert Entity.from_json(sent).typed == UnknownProperties(
        sent["type"], sent["schema_version"], sent["properties"]
    )


@pytest.mark.parametrize("kind", ["organization", "person", "place", "crypto", "weather"])
def test_a_layout_newer_than_this_version_keeps_its_properties(kind: str) -> None:
    current = example(kind)
    newer = {**current, "schema_version": current["schema_version"] + 1}

    assert Entity.from_json(newer).typed == UnknownProperties(
        kind, current["schema_version"] + 1, current["properties"]
    )
    assert not isinstance(Entity.from_json(current).typed, UnknownProperties)


def test_an_enum_value_this_version_does_not_name_reads_as_the_str_sent() -> None:
    sent = example("organization")
    sent["properties"]["funding_rounds"][0]["round_type"] = "series_z"
    sent["properties"]["funding_rounds"][0]["currency"] = "XAU"

    org = Entity.from_json(sent).typed

    assert isinstance(org, OrganizationProperties)
    assert org.funding_rounds[0].round_type == "series_z"
    assert not isinstance(org.funding_rounds[0].round_type, FundingRoundType)
    assert org.funding_rounds[0].currency == "XAU"


def test_null_and_missing_fields_read_as_none() -> None:
    sent = example("organization")
    sent["properties"].update(legal_name=None, headquarters=None, latest_funding_round=None)
    del sent["properties"]["ticker"]

    org = Entity.from_json(sent).typed

    assert isinstance(org, OrganizationProperties)
    assert org.legal_name is None
    assert org.headquarters is None
    assert org.latest_funding_round is None
    assert org.ticker is None


def test_an_instant_with_fractional_seconds_reads() -> None:
    sent = example("crypto")
    sent["properties"]["quote"]["at"] = "2026-11-13T14:58:00.250Z"

    coin = Entity.from_json(sent).typed

    assert isinstance(coin, CryptoProperties)
    assert coin.quote is not None
    assert coin.quote.at == dt.datetime(2026, 11, 13, 14, 58, 0, 250000, tzinfo=dt.UTC)


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("name", None),
        ("founded_year", "1993"),
        ("employee_count", -1),
        ("specialties", "ai"),
        ("funding_rounds", None),
        ("headquarters", "HQ"),
        ("revenue_range", {"min_usd": 10**400, "max_usd": None}),
        ("revenue_range", {"min_usd": float("inf"), "max_usd": None}),
        ("employee_count", 2**64),
    ],
)
def test_properties_off_the_documented_layout_are_a_decode_error(field: str, value: Any) -> None:
    sent = example("organization")
    sent["properties"][field] = value

    with pytest.raises(DecodeError):
        _ = Entity.from_json(sent).typed


def test_a_response_keeps_each_entitys_properties_as_sent() -> None:
    sent = [example("organization"), example("stock")]

    found = SearchResponse.from_json(
        {"as_of": "2026-11-21T00:00:00Z", "request_id": "r", "results": [], "entities": sent}
    )

    assert [e.properties for e in found.entities] == [e["properties"] for e in sent]
