//! Generated from spec/entities.json by python/scripts/entities.py; edit that script, then run it.
//!
//! The typed layout of each public entity type's `properties`, from
//! [`Entity::typed`](super::web::Entity::typed). Every enum is open: a value this
//! version doesn't name reads as `Other`, keeping the value as sent.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

/// A postal address. Any part may be `null`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Address {
    /// The whole address on one line.
    pub formatted: Option<String>,
    /// Street and number.
    pub street: Option<String>,
    /// City or town.
    pub city: Option<String>,
    /// State, province, or equivalent.
    pub region: Option<String>,
    /// Postal code.
    pub postal_code: Option<String>,
    /// ISO 3166-1 alpha-2 country code.
    pub country_code: Option<String>,
}

/// A WMO weather code and its label.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Condition {
    /// The WMO weather code (code table 4677, 0–99).
    pub code: u64,
    /// The code in words, such as `Partly cloudy`.
    pub label: String,
}

/// One UTC day's prices, in its `currency`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CryptoDailyBar {
    /// The UTC day.
    pub date: NaiveDate,
    /// The first price.
    pub open: f64,
    /// The highest price.
    pub high: f64,
    /// The lowest price.
    pub low: f64,
    /// The last price.
    pub close: f64,
    /// The coin traded, in coin units to two decimals.
    pub volume: f64,
    /// The currency of the row's prices.
    pub currency: Currency,
}

/// One UTC day of trading, rolled up from 1-minute bars.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CryptoDay {
    /// The UTC day.
    pub date: NaiveDate,
    /// The price at the start of the day's first bar.
    pub open: f64,
    /// The highest price.
    pub high: f64,
    /// The lowest price.
    pub low: f64,
    /// The price at the end of the day's last bar, or the last price while `partial` is true.
    pub close: f64,
    /// The coin traded, in coin units to two decimals.
    pub volume: f64,
    /// The value traded in `currency`, to two decimals. `null` when the market does not publish it.
    pub quote_volume: Option<f64>,
    /// The number of trades. `null` when the market does not publish it.
    pub trades: Option<u64>,
    /// The currency of the day's prices.
    pub currency: Currency,
    /// The previous UTC day's close. `null` without a previous day.
    pub previous_close: Option<f64>,
    /// `close` minus `previous_close`. `null` without a previous day.
    pub change: Option<f64>,
    /// `change` as a percentage of `previous_close`. `null` without a previous day.
    pub change_pct: Option<f64>,
    /// Whether the day is still in progress at the instant.
    pub partial: bool,
    /// The start of the last minute the day includes.
    pub through: DateTime<Utc>,
}

/// Daily bars, one per UTC day with data. The last bar is the partial day when the range reaches
/// the instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CryptoPriceHistory {
    /// The first date asked for, inclusive.
    pub from: NaiveDate,
    /// The last date asked for, inclusive, clipped to the instant.
    pub to: NaiveDate,
    /// One row per day, oldest first.
    #[serde(default)]
    pub bars: Vec<CryptoDailyBar>,
}

/// The UTC day of the requested `date`, clipped to the instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CryptoPriceOnDate {
    /// The `date` the URL asked for.
    pub requested_date: NaiveDate,
    /// The UTC day returned. This is `requested_date`, or the day that contains the instant when
    /// that day is earlier.
    pub date: NaiveDate,
    /// The price at the start of the day's first bar.
    pub open: f64,
    /// The highest price.
    pub high: f64,
    /// The lowest price.
    pub low: f64,
    /// The price at the end of the day's last bar, or the last price while `partial` is true.
    pub close: f64,
    /// The coin traded, in coin units to two decimals.
    pub volume: f64,
    /// The value traded in `currency`, to two decimals. `null` when the market does not publish it.
    pub quote_volume: Option<f64>,
    /// The number of trades. `null` when the market does not publish it.
    pub trades: Option<u64>,
    /// The currency of the day's prices.
    pub currency: Currency,
    /// The previous UTC day's close. `null` without a previous day.
    pub previous_close: Option<f64>,
    /// `close` minus `previous_close`. `null` without a previous day.
    pub change: Option<f64>,
    /// `change` as a percentage of `previous_close`. `null` without a previous day.
    pub change_pct: Option<f64>,
    /// Whether the day is still in progress at the instant.
    pub partial: bool,
    /// The start of the last minute the day includes.
    pub through: DateTime<Utc>,
}

/// The properties of a crypto asset, which is one coin, not a trading pair. They hold its latest
/// UTC day, the price at a minute, and the price on a date or the daily history that the URL asks
/// for. Each day is rolled up from 1-minute bars of the best market Epochal holds for that day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CryptoProperties {
    /// The asset's name, such as `Bitcoin`.
    pub name: String,
    /// The asset's symbol at the instant, such as `BTC`.
    pub symbol: String,
    /// The currency of `latest` and `range_52w`, such as `USDT`. Every other priced object names
    /// its own currency, and `chart` is in the currency of `quote`.
    pub currency: Currency,
    /// Hours since the last trade (`latest.through`), rounded up, when that trade is more than 24
    /// hours before the instant. The price is then not current. `null` otherwise.
    pub stale_hours: Option<u64>,
    /// The UTC day that contains the instant, up to the instant. `null` when no price is visible.
    pub latest: Option<CryptoDay>,
    /// The low and high of the 365 UTC days to `latest.date`, in `currency`. `null` without `latest`.
    pub range_52w: Option<PriceRange>,
    /// The price at the URL's `at`, or at the latest minute when the URL has no `at`.
    pub quote: Option<CryptoQuote>,
    /// The price every hour over the 24 hours to `quote.at`, in the currency of `quote`. `null`
    /// when no minute in those hours is visible.
    pub chart: Option<PriceChart>,
    /// The UTC day of the URL's `date`, clipped to the instant. `null` without `date`.
    pub price_on_date: Option<CryptoPriceOnDate>,
    /// Daily bars for the URL's `from` and `to`. `null` without `from`.
    pub price_history: Option<CryptoPriceHistory>,
}

/// The price at one minute.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct CryptoQuote {
    /// The start of the minute. This is the URL's `at` rounded down to the minute, or
    /// `latest.through` when the URL has no `at`.
    pub at: DateTime<Utc>,
    /// The price at the end of the minute.
    pub price: f64,
    /// The currency of `price`.
    pub currency: Currency,
}

/// A currency: an ISO 4217 code, or a crypto quote asset such as `USDT`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Currency {
    /// `USD`.
    #[serde(rename = "USD")]
    Usd,
    /// `USDT`.
    #[serde(rename = "USDT")]
    Usdt,
    /// `EUR`.
    #[serde(rename = "EUR")]
    Eur,
    /// `GBP`.
    #[serde(rename = "GBP")]
    Gbp,
    /// `JPY`.
    #[serde(rename = "JPY")]
    Jpy,
    /// `CNY`.
    #[serde(rename = "CNY")]
    Cny,
    /// `CHF`.
    #[serde(rename = "CHF")]
    Chf,
    /// `CAD`.
    #[serde(rename = "CAD")]
    Cad,
    /// `AUD`.
    #[serde(rename = "AUD")]
    Aud,
    /// `NZD`.
    #[serde(rename = "NZD")]
    Nzd,
    /// `HKD`.
    #[serde(rename = "HKD")]
    Hkd,
    /// `SGD`.
    #[serde(rename = "SGD")]
    Sgd,
    /// `KRW`.
    #[serde(rename = "KRW")]
    Krw,
    /// `INR`.
    #[serde(rename = "INR")]
    Inr,
    /// `BRL`.
    #[serde(rename = "BRL")]
    Brl,
    /// `MXN`.
    #[serde(rename = "MXN")]
    Mxn,
    /// `ZAR`.
    #[serde(rename = "ZAR")]
    Zar,
    /// `SEK`.
    #[serde(rename = "SEK")]
    Sek,
    /// `NOK`.
    #[serde(rename = "NOK")]
    Nok,
    /// `DKK`.
    #[serde(rename = "DKK")]
    Dkk,
    /// `PLN`.
    #[serde(rename = "PLN")]
    Pln,
    /// `CZK`.
    #[serde(rename = "CZK")]
    Czk,
    /// `HUF`.
    #[serde(rename = "HUF")]
    Huf,
    /// `RON`.
    #[serde(rename = "RON")]
    Ron,
    /// `TRY`.
    #[serde(rename = "TRY")]
    Try,
    /// `ILS`.
    #[serde(rename = "ILS")]
    Ils,
    /// `IDR`.
    #[serde(rename = "IDR")]
    Idr,
    /// `MYR`.
    #[serde(rename = "MYR")]
    Myr,
    /// `PHP`.
    #[serde(rename = "PHP")]
    Php,
    /// `THB`.
    #[serde(rename = "THB")]
    Thb,
    /// `TWD`.
    #[serde(rename = "TWD")]
    Twd,
    /// `LKR`.
    #[serde(rename = "LKR")]
    Lkr,
    /// `ISK`.
    #[serde(rename = "ISK")]
    Isk,
    /// A value this version doesn't name, as sent.
    #[serde(untagged)]
    Other(String),
}

/// A period of study.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Education {
    /// The school. `null` when unnamed.
    pub school: Option<EntityRef>,
    /// The degree earned, as written, such as `MSEE`. `null` when unknown.
    pub degree: Option<String>,
    /// The field of study, as written. `null` when unknown.
    pub field_of_study: Option<String>,
    /// When the study started. `null` when unknown.
    pub start_date: Option<PartialDate>,
    /// When the study ended. `null` when unknown.
    pub end_date: Option<PartialDate>,
}

/// A reference to another entity or page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct EntityRef {
    /// The entity's URL, such as `https://world.epochal.com/organization/<id>`, or the URL of a
    /// page of one. `null` when the reference has no link, even if the entity has a page.
    pub id: Option<String>,
    /// The name at the instant.
    pub name: String,
}

/// A position a person holds or held.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Experience {
    /// The job title. `null` when unknown.
    pub title: Option<String>,
    /// The employer. `null` when unnamed.
    pub organization: Option<EntityRef>,
    /// When the position started. `null` when unknown.
    pub start_date: Option<PartialDate>,
    /// When the position ended. `null` when ongoing or unknown.
    pub end_date: Option<PartialDate>,
    /// Whether the person holds the position now. `null` when unknown.
    pub is_current: Option<bool>,
    /// Where the position is based, as written. `null` when unknown.
    pub location: Option<String>,
    /// What the role involved, in the person's words. `null` when unknown.
    pub description: Option<String>,
}

/// A funding round.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct FundingRound {
    /// The kind of round. `null` when unknown.
    pub round_type: Option<FundingRoundType>,
    /// The day the round was announced. `null` when unknown.
    pub announced_on: Option<NaiveDate>,
    /// The amount raised, in `currency`. `null` when undisclosed.
    pub amount: Option<f64>,
    /// The currency of `amount`. `null` exactly when `amount` is `null`.
    pub currency: Option<Currency>,
    /// The investors named for the round, in source order.
    #[serde(default)]
    pub investors: Vec<EntityRef>,
}

/// The kind of a funding round, from the source's funding types.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FundingRoundType {
    /// `pre_seed`.
    #[serde(rename = "pre_seed")]
    PreSeed,
    /// `seed`.
    #[serde(rename = "seed")]
    Seed,
    /// `angel`.
    #[serde(rename = "angel")]
    Angel,
    /// `series_a`.
    #[serde(rename = "series_a")]
    SeriesA,
    /// `series_b`.
    #[serde(rename = "series_b")]
    SeriesB,
    /// `series_c`.
    #[serde(rename = "series_c")]
    SeriesC,
    /// `series_d`.
    #[serde(rename = "series_d")]
    SeriesD,
    /// `series_e`.
    #[serde(rename = "series_e")]
    SeriesE,
    /// `series_f`.
    #[serde(rename = "series_f")]
    SeriesF,
    /// `series_g`.
    #[serde(rename = "series_g")]
    SeriesG,
    /// `series_h`.
    #[serde(rename = "series_h")]
    SeriesH,
    /// `series_i`.
    #[serde(rename = "series_i")]
    SeriesI,
    /// `series_j`.
    #[serde(rename = "series_j")]
    SeriesJ,
    /// `series_unknown`.
    #[serde(rename = "series_unknown")]
    SeriesUnknown,
    /// `grant`.
    #[serde(rename = "grant")]
    Grant,
    /// `debt_financing`.
    #[serde(rename = "debt_financing")]
    DebtFinancing,
    /// `convertible_note`.
    #[serde(rename = "convertible_note")]
    ConvertibleNote,
    /// `equity_crowdfunding`.
    #[serde(rename = "equity_crowdfunding")]
    EquityCrowdfunding,
    /// `product_crowdfunding`.
    #[serde(rename = "product_crowdfunding")]
    ProductCrowdfunding,
    /// `private_equity`.
    #[serde(rename = "private_equity")]
    PrivateEquity,
    /// `corporate_round`.
    #[serde(rename = "corporate_round")]
    CorporateRound,
    /// `post_ipo_equity`.
    #[serde(rename = "post_ipo_equity")]
    PostIpoEquity,
    /// `post_ipo_debt`.
    #[serde(rename = "post_ipo_debt")]
    PostIpoDebt,
    /// `post_ipo_secondary`.
    #[serde(rename = "post_ipo_secondary")]
    PostIpoSecondary,
    /// `secondary_market`.
    #[serde(rename = "secondary_market")]
    SecondaryMarket,
    /// `non_equity_assistance`.
    #[serde(rename = "non_equity_assistance")]
    NonEquityAssistance,
    /// `initial_coin_offering`.
    #[serde(rename = "initial_coin_offering")]
    InitialCoinOffering,
    /// `undisclosed`.
    #[serde(rename = "undisclosed")]
    Undisclosed,
    /// A value this version doesn't name, as sent.
    #[serde(untagged)]
    Other(String),
}

/// A point on Earth.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct GeoPoint {
    /// Latitude in degrees, north positive.
    pub lat: f64,
    /// Longitude in degrees, east positive.
    pub lon: f64,
}

/// How well a person speaks a language, from the profile's five levels.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum LanguageProficiency {
    /// `elementary`.
    #[serde(rename = "elementary")]
    Elementary,
    /// `limited_working`.
    #[serde(rename = "limited_working")]
    LimitedWorking,
    /// `professional_working`.
    #[serde(rename = "professional_working")]
    ProfessionalWorking,
    /// `full_professional`.
    #[serde(rename = "full_professional")]
    FullProfessional,
    /// `native_or_bilingual`.
    #[serde(rename = "native_or_bilingual")]
    NativeOrBilingual,
    /// A value this version doesn't name, as sent.
    #[serde(untagged)]
    Other(String),
}

/// The properties of a company.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct OrganizationProperties {
    /// The company's common name, such as `NVIDIA`.
    pub name: String,
    /// The company's full legal name, such as `NVIDIA Corporation`. `null` when unknown.
    pub legal_name: Option<String>,
    /// What the company does, in its own words, as plain text. `null` when unknown.
    pub description: Option<String>,
    /// The company's one-line headline, in its own words. `null` when unknown.
    pub tagline: Option<String>,
    /// The website's host without scheme or path, such as `nvidia.com`. `null` when unknown.
    pub domain: Option<String>,
    /// The primary industry as the profile names it, such as `Computer Hardware Manufacturing`.
    /// `null` when unknown.
    pub industry: Option<String>,
    /// The specialties the company lists, distinct, in profile order.
    #[serde(default)]
    pub specialties: Vec<String>,
    /// The year it was founded. `null` when unknown.
    pub founded_year: Option<u64>,
    /// The number of employees on the profile. `null` when unknown.
    pub employee_count: Option<u64>,
    /// Where the company is headquartered. `null` when no part is known.
    pub headquarters: Option<Address>,
    /// The annual revenue band. `null` when unknown.
    pub revenue_range: Option<RevenueRange>,
    /// The stock ticker of the listing, such as `NVDA`. `null` when not listed or unknown.
    pub ticker: Option<String>,
    /// The listing's exchange code as the profile writes it, such as `NASDAQ`. `null` when unknown.
    pub exchange: Option<String>,
    /// The same round as `funding_rounds[0]`. `null` when `funding_rounds` is empty.
    pub latest_funding_round: Option<FundingRound>,
    /// Dated rounds announced on or before `updated_at`, newest first, without exact duplicates.
    #[serde(default)]
    pub funding_rounds: Vec<FundingRound>,
}

/// A date to the precision the source states: `1993`, `1993-04`, or `1993-04-05`.
pub type PartialDate = String;

/// A language a person speaks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PersonLanguage {
    /// The language, such as `Spanish`.
    pub name: String,
    /// How well the person knows the language. `null` when unknown.
    pub proficiency: Option<LanguageProficiency>,
}

/// Where a person is based.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PersonLocation {
    /// The place as one line, such as `Austin, Texas, United States`. `null` when unknown.
    pub formatted: Option<String>,
    /// ISO 3166-1 alpha-2 country code, uppercase. `null` when unknown.
    pub country_code: Option<String>,
}

/// A post the person published.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PersonPost {
    /// The post's text, or the text it reposts when it adds none.
    pub text: String,
    /// The day it was posted.
    pub posted_on: NaiveDate,
    /// The number of likes. `null` when unknown.
    pub like_count: Option<u64>,
    /// The number of comments. `null` when unknown.
    pub comment_count: Option<u64>,
}

/// The properties of a person.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PersonProperties {
    /// The person's full name.
    pub name: String,
    /// A one-line professional summary. `null` when unknown.
    pub headline: Option<String>,
    /// The person's own description of themselves. `null` when unknown.
    pub summary: Option<String>,
    /// The industry as the profile names it. `null` when unknown.
    pub industry: Option<String>,
    /// Where the person is based. `null` when both parts are unknown.
    pub location: Option<PersonLocation>,
    /// The number of followers of the professional profile. `null` when unknown.
    pub follower_count: Option<u64>,
    /// Positions, current ones first, then the most recent end or start date first.
    #[serde(default)]
    pub experience: Vec<Experience>,
    /// Periods of study, most recent first.
    #[serde(default)]
    pub education: Vec<Education>,
    /// The skills the person lists, distinct, in profile order.
    #[serde(default)]
    pub skills: Vec<String>,
    /// Languages the person speaks, distinct by name.
    #[serde(default)]
    pub languages: Vec<PersonLanguage>,
    /// Recent posts dated on or before `updated_at`, newest first.
    #[serde(default)]
    pub posts: Vec<PersonPost>,
}

/// The hours a place is open on one weekday.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PlaceOpeningHours {
    /// The weekday.
    pub day: Weekday,
    /// The hours as the map writes them, such as `9 am to 12 am`, or `Closed`.
    pub hours: String,
}

/// The properties of a place, which is a business or point of interest, as of the instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PlaceProperties {
    /// The place's name.
    pub name: String,
    /// The main category, as the map labels it, such as `Historical landmark`. It is normally
    /// `categories[0]`. `null` when unknown.
    pub category: Option<String>,
    /// Every category, main one first.
    #[serde(default)]
    pub categories: Vec<String>,
    /// What the place is, in a sentence or two. `null` when unknown.
    pub description: Option<String>,
    /// A longer account of the place, one paragraph per entry.
    #[serde(default)]
    pub summary: Vec<String>,
    /// The street address. `null` when no part is known.
    pub address: Option<Address>,
    /// The place's coordinates. `null` when unknown or out of range.
    pub location: Option<GeoPoint>,
    /// The phone number, as the map lists it. `null` when unknown.
    pub phone: Option<String>,
    /// The place's own website, a full URL. `null` when unknown.
    pub website: Option<String>,
    /// Opening hours per weekday, Monday first, in zero to seven rows.
    #[serde(default)]
    pub opening_hours: Vec<PlaceOpeningHours>,
    /// A summary of the place's reviews. `null` when neither score nor count is known.
    pub rating: Option<PlaceRating>,
    /// Whether the place is open.
    pub status: PlaceStatus,
}

/// A summary of a place's reviews.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PlaceRating {
    /// The average star rating, 1 to 5. `null` when unknown.
    pub score: Option<f64>,
    /// How many reviews the place has. `null` when unknown.
    pub review_count: Option<u64>,
    /// Reviews per star level, from 1 to 5, ascending. It has exactly five rows, or none when unknown.
    #[serde(default)]
    pub distribution: Vec<PlaceStarCount>,
}

/// How many reviews gave one star level.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PlaceStarCount {
    /// The star level, 1 to 5.
    pub stars: u64,
    /// Reviews at that level.
    pub count: u64,
}

/// Whether a place is open.
///
/// - `closed`: Permanently closed.
/// - `removed`: Gone from the map.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PlaceState {
    /// `open`.
    #[serde(rename = "open")]
    Open,
    /// `temporarily_closed`.
    #[serde(rename = "temporarily_closed")]
    TemporarilyClosed,
    /// `closed`.
    #[serde(rename = "closed")]
    Closed,
    /// `removed`.
    #[serde(rename = "removed")]
    Removed,
    /// A value this version doesn't name, as sent.
    #[serde(untagged)]
    Other(String),
}

/// A place's state and, when it is not open, when the map first showed it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PlaceStatus {
    /// Whether the place is open.
    pub state: PlaceState,
    /// When the map first showed the current non-open state. `null` for an open place or when
    /// unknown.
    pub detected_at: Option<DateTime<Utc>>,
}

/// Precipitation in both units.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Precipitation {
    /// Millimeters.
    pub mm: f64,
    /// Inches.
    pub inch: f64,
}

/// Prices, oldest first. The field that holds the chart says what span it covers and how often.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PriceChart {
    /// One price per step that has one, oldest first.
    #[serde(default)]
    pub prices: Vec<PricePoint>,
}

/// A price at a point in time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PricePoint {
    /// The time of the price.
    pub time: DateTime<Utc>,
    /// The price.
    pub price: f64,
}

/// The lowest and highest price over a span.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PriceRange {
    /// The lowest low.
    pub low: f64,
    /// The highest high.
    pub high: f64,
}

/// An annual revenue band, in US dollars per year.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct RevenueRange {
    /// The band's lower bound. `0` for the lowest band.
    pub min_usd: f64,
    /// The band's upper bound. `null` for the open top band.
    pub max_usd: Option<f64>,
}

/// A temperature in both units. The report's own unit is exact, and the other is converted and
/// rounded to 0.1.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Temperature {
    /// Degrees Celsius.
    pub c: f64,
    /// Degrees Fahrenheit.
    pub f: f64,
}

/// The latest observation, or the model's estimate for a place with no station near it or whose
/// station has filed nothing in the last 3 hours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WeatherCurrent {
    /// Observation time, or the time a model estimate is valid for.
    pub observed_at: DateTime<Utc>,
    /// Air temperature.
    pub temperature: Temperature,
    /// Whether the value is a model estimate, used when no station is near or none has filed in 3
    /// hours.
    pub model_estimate: bool,
    /// Relative humidity, in percent. `null` when the report lacks a temperature or dew point.
    pub humidity_pct: Option<f64>,
    /// The wind. `null` when the report has no wind group.
    pub wind: Option<Wind>,
    /// The weather or sky condition. `null` when the report has neither.
    pub condition: Option<Condition>,
}

/// One local day: `today`, `yesterday`, or a day of `days`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WeatherDay {
    /// The local date.
    pub date: NaiveDate,
    /// How final the observed figures are. `null` for a date after the instant.
    pub status: Option<WeatherStatus>,
    /// Whether the figures are a model estimate for a place with no station near it.
    pub model_estimate: bool,
    /// The highest temperature observed.
    pub high: Option<Temperature>,
    /// The lowest temperature observed.
    pub low: Option<Temperature>,
    /// The day's precipitation.
    pub precipitation: Option<Precipitation>,
    /// The day's observations. Filled only for a `?date=` of a day up to the instant.
    pub hourly: Option<Vec<WeatherReading>>,
}

/// The forecast from the newest run published by the instant.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WeatherForecast {
    /// The initialization time of the run that `hourly` comes from.
    pub issued_at: DateTime<Utc>,
    /// The next 48 hours, every 3 hours.
    #[serde(default)]
    pub hourly: Vec<WeatherForecastStep>,
    /// Up to 15 local days from today, each from the newest run that covers it.
    #[serde(default)]
    pub daily: Vec<WeatherForecastDay>,
}

/// The forecast of one local day: a row of `forecast.daily`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WeatherForecastDay {
    /// The local date.
    pub date: NaiveDate,
    /// Forecast high.
    pub high: Temperature,
    /// Forecast low.
    pub low: Temperature,
    /// Forecast precipitation.
    pub precipitation: Precipitation,
    /// The day's most significant weather condition.
    pub condition: Option<Condition>,
}

/// One forecast step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WeatherForecastStep {
    /// The end of the step.
    pub time: DateTime<Utc>,
    /// Temperature at the end of the step.
    pub temperature: Temperature,
    /// Precipitation in the step.
    pub precipitation: Precipitation,
}

/// A place's weather at the instant. It holds the latest report, today's and yesterday's observed
/// figures, and the forecast, or the observed days of a date or range. Today is the local day of
/// the instant. The place is a city, a smaller town, or a landmark. A block is `null` when the
/// view doesn't ask for it or the place has no data for it. No block names the source of its
/// values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WeatherProperties {
    /// The place's name.
    pub name: String,
    /// ISO 3166-1 alpha-2 country code.
    pub country_code: String,
    /// The first-level administrative area, such as a state. `null` when unknown.
    pub region: Option<String>,
    /// The place's coordinates, to two decimals.
    pub location: GeoPoint,
    /// The IANA time zone. Every local date in these properties is in it.
    pub timezone: String,
    /// The latest report. Filled in the default view, and for a `?date=` of today or a later day
    /// that a run covers.
    pub current: Option<WeatherCurrent>,
    /// Today so far. Filled in the default view only.
    pub today: Option<WeatherDay>,
    /// The local day before `today`. Filled in the default view only.
    pub yesterday: Option<WeatherDay>,
    /// The forecast from the newest run published by the instant. Filled in the default view, for a
    /// `?date=` of today or a later day that a run covers, and for a `?from=&to=` that ends after
    /// today, with `daily` cut to the range.
    pub forecast: Option<WeatherForecast>,
    /// The days of `?date=` (one) or `?from=&to=` (each local day of the range up to today),
    /// oldest first. `null` in the default view and for a range that starts after today.
    pub days: Option<Vec<WeatherDay>>,
}

/// One observation of a day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct WeatherReading {
    /// Observation time.
    pub time: DateTime<Utc>,
    /// Air temperature.
    pub temperature: Temperature,
}

/// How final a day's observed figures are.
///
/// - `preliminary`: Issued during the day, before the figure is settled.
/// - `final`: The settled figure, or a correction of it.
/// - `not_reported`: No report is visible at the instant.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum WeatherStatus {
    /// `preliminary`.
    #[serde(rename = "preliminary")]
    Preliminary,
    /// `final`.
    #[serde(rename = "final")]
    Final,
    /// `not_reported`.
    #[serde(rename = "not_reported")]
    NotReported,
    /// A value this version doesn't name, as sent.
    #[serde(untagged)]
    Other(String),
}

/// A day of the week. Values not listed may appear.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Weekday {
    /// `monday`.
    #[serde(rename = "monday")]
    Monday,
    /// `tuesday`.
    #[serde(rename = "tuesday")]
    Tuesday,
    /// `wednesday`.
    #[serde(rename = "wednesday")]
    Wednesday,
    /// `thursday`.
    #[serde(rename = "thursday")]
    Thursday,
    /// `friday`.
    #[serde(rename = "friday")]
    Friday,
    /// `saturday`.
    #[serde(rename = "saturday")]
    Saturday,
    /// `sunday`.
    #[serde(rename = "sunday")]
    Sunday,
    /// A value this version doesn't name, as sent.
    #[serde(untagged)]
    Other(String),
}

/// A wind's speed and direction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Wind {
    /// Sustained speed, km/h.
    pub speed_kmh: f64,
    /// Sustained speed, mph.
    pub speed_mph: f64,
    /// The direction the wind blows from, in degrees clockwise from north. `null` for a calm or
    /// variable wind.
    pub direction_deg: Option<u64>,
}

/// An entity's `properties`, read into the model its `type` names, from
/// [`Entity::typed`](super::web::Entity::typed). It holds `properties` only: `id`,
/// `kind`, `schema_version` and `updated_at` stay on the
/// [`Entity`](super::web::Entity). Fields this version doesn't model are left out;
/// `Entity::properties` keeps them.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum TypedEntity {
    /// `organization`, layout `1`: [`OrganizationProperties`].
    Organization(Box<OrganizationProperties>),
    /// `person`, layout `1`: [`PersonProperties`].
    Person(Box<PersonProperties>),
    /// `crypto`, layout `1`: [`CryptoProperties`].
    Crypto(Box<CryptoProperties>),
    /// `weather`, layout `1`: [`WeatherProperties`].
    Weather(Box<WeatherProperties>),
    /// `place`, layout `1`: [`PlaceProperties`].
    Place(Box<PlaceProperties>),
    /// A `type`, or a `schema_version` of one, that this version doesn't model, with
    /// its `properties` as sent.
    Unknown {
        /// The entity's `type`, as in [`Entity::kind`](super::web::Entity::kind).
        kind: String,
        /// The entity's `schema_version`. For a modeled `kind`, a layout this version
        /// wasn't generated from, older or newer.
        schema_version: u32,
        /// Its `properties`, as sent.
        properties: serde_json::Value,
    },
}

impl TypedEntity {
    pub(crate) fn read(
        kind: &str,
        schema_version: u32,
        properties: &serde_json::Value,
    ) -> Result<TypedEntity, serde_json::Error> {
        Ok(match (kind, schema_version) {
            ("organization", 1) => TypedEntity::Organization(boxed(properties)?),
            ("person", 1) => TypedEntity::Person(boxed(properties)?),
            ("crypto", 1) => TypedEntity::Crypto(boxed(properties)?),
            ("weather", 1) => TypedEntity::Weather(boxed(properties)?),
            ("place", 1) => TypedEntity::Place(boxed(properties)?),
            _ => TypedEntity::Unknown {
                kind: kind.to_owned(),
                schema_version,
                properties: properties.clone(),
            },
        })
    }
}

fn boxed<'de, T: Deserialize<'de>>(
    properties: &'de serde_json::Value,
) -> Result<Box<T>, serde_json::Error> {
    T::deserialize(properties).map(Box::new)
}
