"""The typed layout of each public entity type's ``properties``, from
:attr:`epochal.api.time_portal.Entity.typed`. Every enum is open: a value this version
doesn't name reads as the ``str`` sent.

Generated from spec/entities.json by python/scripts/entities.py; edit that script, then run it.
"""

from __future__ import annotations

import datetime as dt
import enum
import math
from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, Self, TypeAlias, TypeVar

from epochal.api import _wire
from epochal.api._errors import DecodeError

_T = TypeVar("_T")


def _opt(parse: Callable[[Any], _T], value: Any) -> _T | None:
    return None if value is None else parse(value)


def _list(parse: Callable[[Any], _T], values: Any) -> list[_T]:
    if not isinstance(values, list):
        raise TypeError(f"expected a list, got {values!r}")
    return [parse(v) for v in values]


def _str(value: Any) -> str:
    if not isinstance(value, str):
        raise TypeError(f"expected a string, got {value!r}")
    return value


def _int(value: Any) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise TypeError(f"expected an integer, got {value!r}")
    return value


def _uint(value: Any) -> int:
    if not 0 <= _int(value) < 2**64:
        raise ValueError(f"expected an integer in u64 range, got {value!r}")
    return int(value)


def _float(value: Any) -> float:
    if isinstance(value, bool) or not isinstance(value, int | float):
        raise TypeError(f"expected a number, got {value!r}")
    if not math.isfinite(value):
        raise ValueError(f"expected a finite number, got {value!r}")
    return float(value)


def _bool(value: Any) -> bool:
    if not isinstance(value, bool):
        raise TypeError(f"expected a boolean, got {value!r}")
    return value


class _Open(enum.StrEnum):
    """An enum whose values a newer server may add; those read as the ``str`` sent."""

    @classmethod
    def read(cls, value: Any) -> Self | str:
        text = _str(value)
        try:
            return cls(text)
        except ValueError:
            return text


@dataclass(frozen=True, slots=True)
class Address:
    """A postal address. Any part may be `null`."""

    formatted: str | None
    """The whole address on one line."""
    street: str | None
    """Street and number."""
    city: str | None
    """City or town."""
    region: str | None
    """State, province, or equivalent."""
    postal_code: str | None
    """Postal code."""
    country_code: str | None
    """ISO 3166-1 alpha-2 country code."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> Address:
        return cls(
            formatted=_opt(_str, j.get("formatted")),
            street=_opt(_str, j.get("street")),
            city=_opt(_str, j.get("city")),
            region=_opt(_str, j.get("region")),
            postal_code=_opt(_str, j.get("postal_code")),
            country_code=_opt(_str, j.get("country_code")),
        )


@dataclass(frozen=True, slots=True)
class Condition:
    """A WMO weather code and its label."""

    code: int
    """The WMO weather code (code table 4677, 0–99)."""
    label: str
    """The code in words, such as `Partly cloudy`."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> Condition:
        return cls(
            code=_uint(j["code"]),
            label=_str(j["label"]),
        )


@dataclass(frozen=True, slots=True)
class CryptoDailyBar:
    """One UTC day's prices, in its `currency`."""

    date: dt.date
    """The UTC day."""
    open: float
    """The first price."""
    high: float
    """The highest price."""
    low: float
    """The lowest price."""
    close: float
    """The last price."""
    volume: float
    """The coin traded, in coin units to two decimals."""
    currency: Currency | str
    """The currency of the row's prices."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> CryptoDailyBar:
        return cls(
            date=dt.date.fromisoformat(j["date"]),
            open=_float(j["open"]),
            high=_float(j["high"]),
            low=_float(j["low"]),
            close=_float(j["close"]),
            volume=_float(j["volume"]),
            currency=Currency.read(j["currency"]),
        )


@dataclass(frozen=True, slots=True)
class CryptoDay:
    """One UTC day of trading, rolled up from 1-minute bars."""

    date: dt.date
    """The UTC day."""
    open: float
    """The price at the start of the day's first bar."""
    high: float
    """The highest price."""
    low: float
    """The lowest price."""
    close: float
    """The price at the end of the day's last bar, or the last price while `partial` is true."""
    volume: float
    """The coin traded, in coin units to two decimals."""
    quote_volume: float | None
    """The value traded in `currency`, to two decimals. `null` when the market does not publish it."""
    trades: int | None
    """The number of trades. `null` when the market does not publish it."""
    currency: Currency | str
    """The currency of the day's prices."""
    previous_close: float | None
    """The previous UTC day's close. `null` without a previous day."""
    change: float | None
    """`close` minus `previous_close`. `null` without a previous day."""
    change_pct: float | None
    """`change` as a percentage of `previous_close`. `null` without a previous day."""
    partial: bool
    """Whether the day is still in progress at the instant."""
    through: dt.datetime
    """The start of the last minute the day includes."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> CryptoDay:
        return cls(
            date=dt.date.fromisoformat(j["date"]),
            open=_float(j["open"]),
            high=_float(j["high"]),
            low=_float(j["low"]),
            close=_float(j["close"]),
            volume=_float(j["volume"]),
            quote_volume=_opt(_float, j.get("quote_volume")),
            trades=_opt(_uint, j.get("trades")),
            currency=Currency.read(j["currency"]),
            previous_close=_opt(_float, j.get("previous_close")),
            change=_opt(_float, j.get("change")),
            change_pct=_opt(_float, j.get("change_pct")),
            partial=_bool(j["partial"]),
            through=_wire.parse_instant(j["through"]),
        )


@dataclass(frozen=True, slots=True)
class CryptoPriceHistory:
    """Daily bars, one per UTC day with data. The last bar is the partial day when the range reaches
    the instant."""

    from_: dt.date
    """The first date asked for, inclusive."""
    to: dt.date
    """The last date asked for, inclusive, clipped to the instant."""
    bars: list[CryptoDailyBar]
    """One row per day, oldest first."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> CryptoPriceHistory:
        return cls(
            from_=dt.date.fromisoformat(j["from"]),
            to=dt.date.fromisoformat(j["to"]),
            bars=_list(CryptoDailyBar.from_json, j.get("bars", [])),
        )


@dataclass(frozen=True, slots=True)
class CryptoPriceOnDate:
    """The UTC day of the requested `date`, clipped to the instant."""

    requested_date: dt.date
    """The `date` the URL asked for."""
    date: dt.date
    """The UTC day returned. This is `requested_date`, or the day that contains the instant when
    that day is earlier."""
    open: float
    """The price at the start of the day's first bar."""
    high: float
    """The highest price."""
    low: float
    """The lowest price."""
    close: float
    """The price at the end of the day's last bar, or the last price while `partial` is true."""
    volume: float
    """The coin traded, in coin units to two decimals."""
    quote_volume: float | None
    """The value traded in `currency`, to two decimals. `null` when the market does not publish it."""
    trades: int | None
    """The number of trades. `null` when the market does not publish it."""
    currency: Currency | str
    """The currency of the day's prices."""
    previous_close: float | None
    """The previous UTC day's close. `null` without a previous day."""
    change: float | None
    """`close` minus `previous_close`. `null` without a previous day."""
    change_pct: float | None
    """`change` as a percentage of `previous_close`. `null` without a previous day."""
    partial: bool
    """Whether the day is still in progress at the instant."""
    through: dt.datetime
    """The start of the last minute the day includes."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> CryptoPriceOnDate:
        return cls(
            requested_date=dt.date.fromisoformat(j["requested_date"]),
            date=dt.date.fromisoformat(j["date"]),
            open=_float(j["open"]),
            high=_float(j["high"]),
            low=_float(j["low"]),
            close=_float(j["close"]),
            volume=_float(j["volume"]),
            quote_volume=_opt(_float, j.get("quote_volume")),
            trades=_opt(_uint, j.get("trades")),
            currency=Currency.read(j["currency"]),
            previous_close=_opt(_float, j.get("previous_close")),
            change=_opt(_float, j.get("change")),
            change_pct=_opt(_float, j.get("change_pct")),
            partial=_bool(j["partial"]),
            through=_wire.parse_instant(j["through"]),
        )


@dataclass(frozen=True, slots=True)
class CryptoProperties:
    """The properties of a crypto asset, which is one coin, not a trading pair. They hold its latest
    UTC day, the price at a minute, and the price on a date or the daily history that the URL asks
    for. Each day is rolled up from 1-minute bars of the best market Epochal holds for that day."""

    name: str
    """The asset's name, such as `Bitcoin`."""
    symbol: str
    """The asset's symbol at the instant, such as `BTC`."""
    currency: Currency | str
    """The currency of `latest` and `range_52w`, such as `USDT`. Every other priced object names
    its own currency, and `chart` is in the currency of `quote`."""
    stale_hours: int | None
    """Hours since the last trade (`latest.through`), rounded up, when that trade is more than 24
    hours before the instant. The price is then not current. `null` otherwise."""
    latest: CryptoDay | None
    """The UTC day that contains the instant, up to the instant. `null` when no price is visible."""
    range_52w: PriceRange | None
    """The low and high of the 365 UTC days to `latest.date`, in `currency`. `null` without `latest`."""
    quote: CryptoQuote | None
    """The price at the URL's `at`, or at the latest minute when the URL has no `at`."""
    chart: PriceChart | None
    """The price every hour over the 24 hours to `quote.at`, in the currency of `quote`. `null`
    when no minute in those hours is visible."""
    price_on_date: CryptoPriceOnDate | None
    """The UTC day of the URL's `date`, clipped to the instant. `null` without `date`."""
    price_history: CryptoPriceHistory | None
    """Daily bars for the URL's `from` and `to`. `null` without `from`."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> CryptoProperties:
        return cls(
            name=_str(j["name"]),
            symbol=_str(j["symbol"]),
            currency=Currency.read(j["currency"]),
            stale_hours=_opt(_uint, j.get("stale_hours")),
            latest=_opt(CryptoDay.from_json, j.get("latest")),
            range_52w=_opt(PriceRange.from_json, j.get("range_52w")),
            quote=_opt(CryptoQuote.from_json, j.get("quote")),
            chart=_opt(PriceChart.from_json, j.get("chart")),
            price_on_date=_opt(CryptoPriceOnDate.from_json, j.get("price_on_date")),
            price_history=_opt(CryptoPriceHistory.from_json, j.get("price_history")),
        )


@dataclass(frozen=True, slots=True)
class CryptoQuote:
    """The price at one minute."""

    at: dt.datetime
    """The start of the minute. This is the URL's `at` rounded down to the minute, or
    `latest.through` when the URL has no `at`."""
    price: float
    """The price at the end of the minute."""
    currency: Currency | str
    """The currency of `price`."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> CryptoQuote:
        return cls(
            at=_wire.parse_instant(j["at"]),
            price=_float(j["price"]),
            currency=Currency.read(j["currency"]),
        )


class Currency(_Open):
    """A currency: an ISO 4217 code, or a crypto quote asset such as `USDT`."""

    USD = "USD"
    USDT = "USDT"
    EUR = "EUR"
    GBP = "GBP"
    JPY = "JPY"
    CNY = "CNY"
    CHF = "CHF"
    CAD = "CAD"
    AUD = "AUD"
    NZD = "NZD"
    HKD = "HKD"
    SGD = "SGD"
    KRW = "KRW"
    INR = "INR"
    BRL = "BRL"
    MXN = "MXN"
    ZAR = "ZAR"
    SEK = "SEK"
    NOK = "NOK"
    DKK = "DKK"
    PLN = "PLN"
    CZK = "CZK"
    HUF = "HUF"
    RON = "RON"
    TRY = "TRY"
    ILS = "ILS"
    IDR = "IDR"
    MYR = "MYR"
    PHP = "PHP"
    THB = "THB"
    TWD = "TWD"
    LKR = "LKR"
    ISK = "ISK"


@dataclass(frozen=True, slots=True)
class Education:
    """A period of study."""

    school: EntityRef | None
    """The school. `null` when unnamed."""
    degree: str | None
    """The degree earned, as written, such as `MSEE`. `null` when unknown."""
    field_of_study: str | None
    """The field of study, as written. `null` when unknown."""
    start_date: PartialDate | None
    """When the study started. `null` when unknown."""
    end_date: PartialDate | None
    """When the study ended. `null` when unknown."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> Education:
        return cls(
            school=_opt(EntityRef.from_json, j.get("school")),
            degree=_opt(_str, j.get("degree")),
            field_of_study=_opt(_str, j.get("field_of_study")),
            start_date=_opt(_str, j.get("start_date")),
            end_date=_opt(_str, j.get("end_date")),
        )


@dataclass(frozen=True, slots=True)
class EntityRef:
    """A reference to another entity or page."""

    id: str | None
    """The entity's URL, such as `https://world.epochal.com/organization/<id>`, or the URL of a
    page of one. `null` when the reference has no link, even if the entity has a page."""
    name: str
    """The name at the instant."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> EntityRef:
        return cls(
            id=_opt(_str, j.get("id")),
            name=_str(j["name"]),
        )


@dataclass(frozen=True, slots=True)
class Experience:
    """A position a person holds or held."""

    title: str | None
    """The job title. `null` when unknown."""
    organization: EntityRef | None
    """The employer. `null` when unnamed."""
    start_date: PartialDate | None
    """When the position started. `null` when unknown."""
    end_date: PartialDate | None
    """When the position ended. `null` when ongoing or unknown."""
    is_current: bool | None
    """Whether the person holds the position now. `null` when unknown."""
    location: str | None
    """Where the position is based, as written. `null` when unknown."""
    description: str | None
    """What the role involved, in the person's words. `null` when unknown."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> Experience:
        return cls(
            title=_opt(_str, j.get("title")),
            organization=_opt(EntityRef.from_json, j.get("organization")),
            start_date=_opt(_str, j.get("start_date")),
            end_date=_opt(_str, j.get("end_date")),
            is_current=_opt(_bool, j.get("is_current")),
            location=_opt(_str, j.get("location")),
            description=_opt(_str, j.get("description")),
        )


@dataclass(frozen=True, slots=True)
class FundingRound:
    """A funding round."""

    round_type: FundingRoundType | str | None
    """The kind of round. `null` when unknown."""
    announced_on: dt.date | None
    """The day the round was announced. `null` when unknown."""
    amount: float | None
    """The amount raised, in `currency`. `null` when undisclosed."""
    currency: Currency | str | None
    """The currency of `amount`. `null` exactly when `amount` is `null`."""
    investors: list[EntityRef]
    """The investors named for the round, in source order."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> FundingRound:
        return cls(
            round_type=_opt(FundingRoundType.read, j.get("round_type")),
            announced_on=_opt(dt.date.fromisoformat, j.get("announced_on")),
            amount=_opt(_float, j.get("amount")),
            currency=_opt(Currency.read, j.get("currency")),
            investors=_list(EntityRef.from_json, j.get("investors", [])),
        )


class FundingRoundType(_Open):
    """The kind of a funding round, from the source's funding types."""

    PRE_SEED = "pre_seed"
    SEED = "seed"
    ANGEL = "angel"
    SERIES_A = "series_a"
    SERIES_B = "series_b"
    SERIES_C = "series_c"
    SERIES_D = "series_d"
    SERIES_E = "series_e"
    SERIES_F = "series_f"
    SERIES_G = "series_g"
    SERIES_H = "series_h"
    SERIES_I = "series_i"
    SERIES_J = "series_j"
    SERIES_UNKNOWN = "series_unknown"
    GRANT = "grant"
    DEBT_FINANCING = "debt_financing"
    CONVERTIBLE_NOTE = "convertible_note"
    EQUITY_CROWDFUNDING = "equity_crowdfunding"
    PRODUCT_CROWDFUNDING = "product_crowdfunding"
    PRIVATE_EQUITY = "private_equity"
    CORPORATE_ROUND = "corporate_round"
    POST_IPO_EQUITY = "post_ipo_equity"
    POST_IPO_DEBT = "post_ipo_debt"
    POST_IPO_SECONDARY = "post_ipo_secondary"
    SECONDARY_MARKET = "secondary_market"
    NON_EQUITY_ASSISTANCE = "non_equity_assistance"
    INITIAL_COIN_OFFERING = "initial_coin_offering"
    UNDISCLOSED = "undisclosed"


@dataclass(frozen=True, slots=True)
class GeoPoint:
    """A point on Earth."""

    lat: float
    """Latitude in degrees, north positive."""
    lon: float
    """Longitude in degrees, east positive."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> GeoPoint:
        return cls(
            lat=_float(j["lat"]),
            lon=_float(j["lon"]),
        )


class LanguageProficiency(_Open):
    """How well a person speaks a language, from the profile's five levels."""

    ELEMENTARY = "elementary"
    LIMITED_WORKING = "limited_working"
    PROFESSIONAL_WORKING = "professional_working"
    FULL_PROFESSIONAL = "full_professional"
    NATIVE_OR_BILINGUAL = "native_or_bilingual"


@dataclass(frozen=True, slots=True)
class OrganizationProperties:
    """The properties of a company."""

    name: str
    """The company's common name, such as `NVIDIA`."""
    legal_name: str | None
    """The company's full legal name, such as `NVIDIA Corporation`. `null` when unknown."""
    description: str | None
    """What the company does, in its own words, as plain text. `null` when unknown."""
    tagline: str | None
    """The company's one-line headline, in its own words. `null` when unknown."""
    domain: str | None
    """The website's host without scheme or path, such as `nvidia.com`. `null` when unknown."""
    industry: str | None
    """The primary industry as the profile names it, such as `Computer Hardware Manufacturing`.
    `null` when unknown."""
    specialties: list[str]
    """The specialties the company lists, distinct, in profile order."""
    founded_year: int | None
    """The year it was founded. `null` when unknown."""
    employee_count: int | None
    """The number of employees on the profile. `null` when unknown."""
    headquarters: Address | None
    """Where the company is headquartered. `null` when no part is known."""
    revenue_range: RevenueRange | None
    """The annual revenue band. `null` when unknown."""
    ticker: str | None
    """The stock ticker of the listing, such as `NVDA`. `null` when not listed or unknown."""
    exchange: str | None
    """The listing's exchange code as the profile writes it, such as `NASDAQ`. `null` when unknown."""
    latest_funding_round: FundingRound | None
    """The same round as `funding_rounds[0]`. `null` when `funding_rounds` is empty."""
    funding_rounds: list[FundingRound]
    """Dated rounds announced on or before `updated_at`, newest first, without exact duplicates."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> OrganizationProperties:
        return cls(
            name=_str(j["name"]),
            legal_name=_opt(_str, j.get("legal_name")),
            description=_opt(_str, j.get("description")),
            tagline=_opt(_str, j.get("tagline")),
            domain=_opt(_str, j.get("domain")),
            industry=_opt(_str, j.get("industry")),
            specialties=_list(_str, j.get("specialties", [])),
            founded_year=_opt(_uint, j.get("founded_year")),
            employee_count=_opt(_uint, j.get("employee_count")),
            headquarters=_opt(Address.from_json, j.get("headquarters")),
            revenue_range=_opt(RevenueRange.from_json, j.get("revenue_range")),
            ticker=_opt(_str, j.get("ticker")),
            exchange=_opt(_str, j.get("exchange")),
            latest_funding_round=_opt(FundingRound.from_json, j.get("latest_funding_round")),
            funding_rounds=_list(FundingRound.from_json, j.get("funding_rounds", [])),
        )


PartialDate: TypeAlias = str
"""A date to the precision the source states: `1993`, `1993-04`, or `1993-04-05`."""


@dataclass(frozen=True, slots=True)
class PersonLanguage:
    """A language a person speaks."""

    name: str
    """The language, such as `Spanish`."""
    proficiency: LanguageProficiency | str | None
    """How well the person knows the language. `null` when unknown."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PersonLanguage:
        return cls(
            name=_str(j["name"]),
            proficiency=_opt(LanguageProficiency.read, j.get("proficiency")),
        )


@dataclass(frozen=True, slots=True)
class PersonLocation:
    """Where a person is based."""

    formatted: str | None
    """The place as one line, such as `Austin, Texas, United States`. `null` when unknown."""
    country_code: str | None
    """ISO 3166-1 alpha-2 country code, uppercase. `null` when unknown."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PersonLocation:
        return cls(
            formatted=_opt(_str, j.get("formatted")),
            country_code=_opt(_str, j.get("country_code")),
        )


@dataclass(frozen=True, slots=True)
class PersonPost:
    """A post the person published."""

    text: str
    """The post's text, or the text it reposts when it adds none."""
    posted_on: dt.date
    """The day it was posted."""
    like_count: int | None
    """The number of likes. `null` when unknown."""
    comment_count: int | None
    """The number of comments. `null` when unknown."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PersonPost:
        return cls(
            text=_str(j["text"]),
            posted_on=dt.date.fromisoformat(j["posted_on"]),
            like_count=_opt(_uint, j.get("like_count")),
            comment_count=_opt(_uint, j.get("comment_count")),
        )


@dataclass(frozen=True, slots=True)
class PersonProperties:
    """The properties of a person."""

    name: str
    """The person's full name."""
    headline: str | None
    """A one-line professional summary. `null` when unknown."""
    summary: str | None
    """The person's own description of themselves. `null` when unknown."""
    industry: str | None
    """The industry as the profile names it. `null` when unknown."""
    location: PersonLocation | None
    """Where the person is based. `null` when both parts are unknown."""
    follower_count: int | None
    """The number of followers of the professional profile. `null` when unknown."""
    experience: list[Experience]
    """Positions, current ones first, then the most recent end or start date first."""
    education: list[Education]
    """Periods of study, most recent first."""
    skills: list[str]
    """The skills the person lists, distinct, in profile order."""
    languages: list[PersonLanguage]
    """Languages the person speaks, distinct by name."""
    posts: list[PersonPost]
    """Recent posts dated on or before `updated_at`, newest first."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PersonProperties:
        return cls(
            name=_str(j["name"]),
            headline=_opt(_str, j.get("headline")),
            summary=_opt(_str, j.get("summary")),
            industry=_opt(_str, j.get("industry")),
            location=_opt(PersonLocation.from_json, j.get("location")),
            follower_count=_opt(_uint, j.get("follower_count")),
            experience=_list(Experience.from_json, j.get("experience", [])),
            education=_list(Education.from_json, j.get("education", [])),
            skills=_list(_str, j.get("skills", [])),
            languages=_list(PersonLanguage.from_json, j.get("languages", [])),
            posts=_list(PersonPost.from_json, j.get("posts", [])),
        )


@dataclass(frozen=True, slots=True)
class PlaceOpeningHours:
    """The hours a place is open on one weekday."""

    day: Weekday | str
    """The weekday."""
    hours: str
    """The hours as the map writes them, such as `9 am to 12 am`, or `Closed`."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PlaceOpeningHours:
        return cls(
            day=Weekday.read(j["day"]),
            hours=_str(j["hours"]),
        )


@dataclass(frozen=True, slots=True)
class PlaceProperties:
    """The properties of a place, which is a business or point of interest, as of the instant."""

    name: str
    """The place's name."""
    category: str | None
    """The main category, as the map labels it, such as `Historical landmark`. It is normally
    `categories[0]`. `null` when unknown."""
    categories: list[str]
    """Every category, main one first."""
    description: str | None
    """What the place is, in a sentence or two. `null` when unknown."""
    summary: list[str]
    """A longer account of the place, one paragraph per entry."""
    address: Address | None
    """The street address. `null` when no part is known."""
    location: GeoPoint | None
    """The place's coordinates. `null` when unknown or out of range."""
    phone: str | None
    """The phone number, as the map lists it. `null` when unknown."""
    website: str | None
    """The place's own website, a full URL. `null` when unknown."""
    opening_hours: list[PlaceOpeningHours]
    """Opening hours per weekday, Monday first, in zero to seven rows."""
    rating: PlaceRating | None
    """A summary of the place's reviews. `null` when neither score nor count is known."""
    status: PlaceStatus
    """Whether the place is open."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PlaceProperties:
        return cls(
            name=_str(j["name"]),
            category=_opt(_str, j.get("category")),
            categories=_list(_str, j.get("categories", [])),
            description=_opt(_str, j.get("description")),
            summary=_list(_str, j.get("summary", [])),
            address=_opt(Address.from_json, j.get("address")),
            location=_opt(GeoPoint.from_json, j.get("location")),
            phone=_opt(_str, j.get("phone")),
            website=_opt(_str, j.get("website")),
            opening_hours=_list(PlaceOpeningHours.from_json, j.get("opening_hours", [])),
            rating=_opt(PlaceRating.from_json, j.get("rating")),
            status=PlaceStatus.from_json(j["status"]),
        )


@dataclass(frozen=True, slots=True)
class PlaceRating:
    """A summary of a place's reviews."""

    score: float | None
    """The average star rating, 1 to 5. `null` when unknown."""
    review_count: int | None
    """How many reviews the place has. `null` when unknown."""
    distribution: list[PlaceStarCount]
    """Reviews per star level, from 1 to 5, ascending. It has exactly five rows, or none when unknown."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PlaceRating:
        return cls(
            score=_opt(_float, j.get("score")),
            review_count=_opt(_uint, j.get("review_count")),
            distribution=_list(PlaceStarCount.from_json, j.get("distribution", [])),
        )


@dataclass(frozen=True, slots=True)
class PlaceStarCount:
    """How many reviews gave one star level."""

    stars: int
    """The star level, 1 to 5."""
    count: int
    """Reviews at that level."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PlaceStarCount:
        return cls(
            stars=_uint(j["stars"]),
            count=_uint(j["count"]),
        )


class PlaceState(_Open):
    """Whether a place is open.

    - `closed`: Permanently closed.
    - `removed`: Gone from the map."""

    OPEN = "open"
    TEMPORARILY_CLOSED = "temporarily_closed"
    CLOSED = "closed"
    REMOVED = "removed"


@dataclass(frozen=True, slots=True)
class PlaceStatus:
    """A place's state and, when it is not open, when the map first showed it."""

    state: PlaceState | str
    """Whether the place is open."""
    detected_at: dt.datetime | None
    """When the map first showed the current non-open state. `null` for an open place or when
    unknown."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PlaceStatus:
        return cls(
            state=PlaceState.read(j["state"]),
            detected_at=_opt(_wire.parse_instant, j.get("detected_at")),
        )


@dataclass(frozen=True, slots=True)
class Precipitation:
    """Precipitation in both units."""

    mm: float
    """Millimeters."""
    inch: float
    """Inches."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> Precipitation:
        return cls(
            mm=_float(j["mm"]),
            inch=_float(j["inch"]),
        )


@dataclass(frozen=True, slots=True)
class PriceChart:
    """Prices, oldest first. The field that holds the chart says what span it covers and how often."""

    prices: list[PricePoint]
    """One price per step that has one, oldest first."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PriceChart:
        return cls(
            prices=_list(PricePoint.from_json, j.get("prices", [])),
        )


@dataclass(frozen=True, slots=True)
class PricePoint:
    """A price at a point in time."""

    time: dt.datetime
    """The time of the price."""
    price: float
    """The price."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PricePoint:
        return cls(
            time=_wire.parse_instant(j["time"]),
            price=_float(j["price"]),
        )


@dataclass(frozen=True, slots=True)
class PriceRange:
    """The lowest and highest price over a span."""

    low: float
    """The lowest low."""
    high: float
    """The highest high."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> PriceRange:
        return cls(
            low=_float(j["low"]),
            high=_float(j["high"]),
        )


@dataclass(frozen=True, slots=True)
class RevenueRange:
    """An annual revenue band, in US dollars per year."""

    min_usd: float
    """The band's lower bound. `0` for the lowest band."""
    max_usd: float | None
    """The band's upper bound. `null` for the open top band."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> RevenueRange:
        return cls(
            min_usd=_float(j["min_usd"]),
            max_usd=_opt(_float, j.get("max_usd")),
        )


@dataclass(frozen=True, slots=True)
class Temperature:
    """A temperature in both units. The report's own unit is exact, and the other is converted and
    rounded to 0.1."""

    c: float
    """Degrees Celsius."""
    f: float
    """Degrees Fahrenheit."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> Temperature:
        return cls(
            c=_float(j["c"]),
            f=_float(j["f"]),
        )


@dataclass(frozen=True, slots=True)
class WeatherCurrent:
    """The latest observation, or the model's estimate for a place with no station near it or whose
    station has filed nothing in the last 3 hours."""

    observed_at: dt.datetime
    """Observation time, or the time a model estimate is valid for."""
    temperature: Temperature
    """Air temperature."""
    model_estimate: bool
    """Whether the value is a model estimate, used when no station is near or none has filed in 3
    hours."""
    humidity_pct: float | None
    """Relative humidity, in percent. `null` when the report lacks a temperature or dew point."""
    wind: Wind | None
    """The wind. `null` when the report has no wind group."""
    condition: Condition | None
    """The weather or sky condition. `null` when the report has neither."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> WeatherCurrent:
        return cls(
            observed_at=_wire.parse_instant(j["observed_at"]),
            temperature=Temperature.from_json(j["temperature"]),
            model_estimate=_bool(j["model_estimate"]),
            humidity_pct=_opt(_float, j.get("humidity_pct")),
            wind=_opt(Wind.from_json, j.get("wind")),
            condition=_opt(Condition.from_json, j.get("condition")),
        )


@dataclass(frozen=True, slots=True)
class WeatherDay:
    """One local day: `today`, `yesterday`, or a day of `days`."""

    date: dt.date
    """The local date."""
    status: WeatherStatus | str | None
    """How final the observed figures are. `null` for a date after the instant."""
    model_estimate: bool
    """Whether the figures are a model estimate for a place with no station near it."""
    high: Temperature | None
    """The highest temperature observed."""
    low: Temperature | None
    """The lowest temperature observed."""
    precipitation: Precipitation | None
    """The day's precipitation."""
    hourly: list[WeatherReading] | None
    """The day's observations. Filled only for a `?date=` of a day up to the instant."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> WeatherDay:
        return cls(
            date=dt.date.fromisoformat(j["date"]),
            status=_opt(WeatherStatus.read, j.get("status")),
            model_estimate=_bool(j["model_estimate"]),
            high=_opt(Temperature.from_json, j.get("high")),
            low=_opt(Temperature.from_json, j.get("low")),
            precipitation=_opt(Precipitation.from_json, j.get("precipitation")),
            hourly=_opt(lambda v: _list(WeatherReading.from_json, v), j.get("hourly")),
        )


@dataclass(frozen=True, slots=True)
class WeatherForecast:
    """The forecast from the newest run published by the instant."""

    issued_at: dt.datetime
    """The initialization time of the run that `hourly` comes from."""
    hourly: list[WeatherForecastStep]
    """The next 48 hours, every 3 hours."""
    daily: list[WeatherForecastDay]
    """Up to 15 local days from today, each from the newest run that covers it."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> WeatherForecast:
        return cls(
            issued_at=_wire.parse_instant(j["issued_at"]),
            hourly=_list(WeatherForecastStep.from_json, j.get("hourly", [])),
            daily=_list(WeatherForecastDay.from_json, j.get("daily", [])),
        )


@dataclass(frozen=True, slots=True)
class WeatherForecastDay:
    """The forecast of one local day: a row of `forecast.daily`."""

    date: dt.date
    """The local date."""
    high: Temperature
    """Forecast high."""
    low: Temperature
    """Forecast low."""
    precipitation: Precipitation
    """Forecast precipitation."""
    condition: Condition | None
    """The day's most significant weather condition."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> WeatherForecastDay:
        return cls(
            date=dt.date.fromisoformat(j["date"]),
            high=Temperature.from_json(j["high"]),
            low=Temperature.from_json(j["low"]),
            precipitation=Precipitation.from_json(j["precipitation"]),
            condition=_opt(Condition.from_json, j.get("condition")),
        )


@dataclass(frozen=True, slots=True)
class WeatherForecastStep:
    """One forecast step."""

    time: dt.datetime
    """The end of the step."""
    temperature: Temperature
    """Temperature at the end of the step."""
    precipitation: Precipitation
    """Precipitation in the step."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> WeatherForecastStep:
        return cls(
            time=_wire.parse_instant(j["time"]),
            temperature=Temperature.from_json(j["temperature"]),
            precipitation=Precipitation.from_json(j["precipitation"]),
        )


@dataclass(frozen=True, slots=True)
class WeatherProperties:
    """A place's weather at the instant. It holds the latest report, today's and yesterday's observed
    figures, and the forecast, or the observed days of a date or range. Today is the local day of
    the instant. The place is a city, a smaller town, or a landmark. A block is `null` when the
    view doesn't ask for it or the place has no data for it. No block names the source of its
    values."""

    name: str
    """The place's name."""
    country_code: str
    """ISO 3166-1 alpha-2 country code."""
    region: str | None
    """The first-level administrative area, such as a state. `null` when unknown."""
    location: GeoPoint
    """The place's coordinates, to two decimals."""
    timezone: str
    """The IANA time zone. Every local date in these properties is in it."""
    current: WeatherCurrent | None
    """The latest report. Filled in the default view, and for a `?date=` of today or a later day
    that a run covers."""
    today: WeatherDay | None
    """Today so far. Filled in the default view only."""
    yesterday: WeatherDay | None
    """The local day before `today`. Filled in the default view only."""
    forecast: WeatherForecast | None
    """The forecast from the newest run published by the instant. Filled in the default view, for a
    `?date=` of today or a later day that a run covers, and for a `?from=&to=` that ends after
    today, with `daily` cut to the range."""
    days: list[WeatherDay] | None
    """The days of `?date=` (one) or `?from=&to=` (each local day of the range up to today),
    oldest first. `null` in the default view and for a range that starts after today."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> WeatherProperties:
        return cls(
            name=_str(j["name"]),
            country_code=_str(j["country_code"]),
            region=_opt(_str, j.get("region")),
            location=GeoPoint.from_json(j["location"]),
            timezone=_str(j["timezone"]),
            current=_opt(WeatherCurrent.from_json, j.get("current")),
            today=_opt(WeatherDay.from_json, j.get("today")),
            yesterday=_opt(WeatherDay.from_json, j.get("yesterday")),
            forecast=_opt(WeatherForecast.from_json, j.get("forecast")),
            days=_opt(lambda v: _list(WeatherDay.from_json, v), j.get("days")),
        )


@dataclass(frozen=True, slots=True)
class WeatherReading:
    """One observation of a day."""

    time: dt.datetime
    """Observation time."""
    temperature: Temperature
    """Air temperature."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> WeatherReading:
        return cls(
            time=_wire.parse_instant(j["time"]),
            temperature=Temperature.from_json(j["temperature"]),
        )


class WeatherStatus(_Open):
    """How final a day's observed figures are.

    - `preliminary`: Issued during the day, before the figure is settled.
    - `final`: The settled figure, or a correction of it.
    - `not_reported`: No report is visible at the instant."""

    PRELIMINARY = "preliminary"
    FINAL = "final"
    NOT_REPORTED = "not_reported"


class Weekday(_Open):
    """A day of the week. Values not listed may appear."""

    MONDAY = "monday"
    TUESDAY = "tuesday"
    WEDNESDAY = "wednesday"
    THURSDAY = "thursday"
    FRIDAY = "friday"
    SATURDAY = "saturday"
    SUNDAY = "sunday"


@dataclass(frozen=True, slots=True)
class Wind:
    """A wind's speed and direction."""

    speed_kmh: float
    """Sustained speed, km/h."""
    speed_mph: float
    """Sustained speed, mph."""
    direction_deg: int | None
    """The direction the wind blows from, in degrees clockwise from north. `null` for a calm or
    variable wind."""

    @classmethod
    def from_json(cls, j: dict[str, Any]) -> Wind:
        return cls(
            speed_kmh=_float(j["speed_kmh"]),
            speed_mph=_float(j["speed_mph"]),
            direction_deg=_opt(_uint, j.get("direction_deg")),
        )


@dataclass(frozen=True, slots=True)
class UnknownProperties:
    """The ``properties`` of a ``type``, or a ``schema_version`` of one, that this
    version doesn't model, as sent. ``type`` and ``schema_version`` are the entity's; a
    modeled ``type`` with another ``schema_version`` is a layout this version wasn't
    generated from, older or newer."""

    type: str
    schema_version: int
    properties: dict[str, Any]


TypedEntity: TypeAlias = (
    OrganizationProperties
    | PersonProperties
    | CryptoProperties
    | WeatherProperties
    | PlaceProperties
    | UnknownProperties
)
"""An entity's ``properties``, read into the class its ``type`` names. It holds
``properties`` only: ``id``, ``type``, ``schema_version`` and ``updated_at`` stay on the
:class:`~epochal.api.time_portal.Entity`. Fields this version doesn't model are left
out; ``Entity.properties`` keeps them."""

_READERS: dict[tuple[str, int], Callable[[dict[str, Any]], TypedEntity]] = {
    ("organization", 1): OrganizationProperties.from_json,
    ("person", 1): PersonProperties.from_json,
    ("crypto", 1): CryptoProperties.from_json,
    ("weather", 1): WeatherProperties.from_json,
    ("place", 1): PlaceProperties.from_json,
}


def typed(kind: str, schema_version: int, properties: dict[str, Any]) -> TypedEntity:
    """``properties`` read into the class of ``kind`` at layout ``schema_version``;
    :class:`UnknownProperties` for a kind or layout this version doesn't model. Raises
    :class:`~epochal.api.DecodeError` when they don't match the documented layout."""
    read = _READERS.get((kind, schema_version))
    if read is None:
        return UnknownProperties(kind, schema_version, properties)
    try:
        return read(properties)
    except (KeyError, TypeError, ValueError, OverflowError, AttributeError) as e:
        raise DecodeError(f"unreadable {kind} properties: {e!r}") from e
