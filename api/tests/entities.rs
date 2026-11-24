//! `Entity::typed` on the guide's example of each type, copied into `spec/examples`.

use epochal_api::chrono::{DateTime, NaiveDate, Utc};
use epochal_api::time_portal::entities::{
    Currency, FundingRoundType, PlaceState, TypedEntity, WeatherStatus, Weekday,
};
use epochal_api::time_portal::web::{Entity, SearchResponse};
use serde_json::{Value, json};

const ORGANIZATION: &str = include_str!("../../spec/examples/organization.json");
const PERSON: &str = include_str!("../../spec/examples/person.json");
const PLACE: &str = include_str!("../../spec/examples/place.json");
const CRYPTO: &str = include_str!("../../spec/examples/crypto.json");
const WEATHER: &str = include_str!("../../spec/examples/weather.json");
const STOCK: &str = include_str!("../../spec/examples/stock.json");

fn example(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

fn entity(value: Value) -> Entity {
    serde_json::from_value(value).unwrap()
}

fn instant(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

#[test]
fn an_organization_reads_into_its_model() {
    let TypedEntity::Organization(org) = entity(example(ORGANIZATION)).typed().unwrap() else {
        panic!("not an organization");
    };
    assert_eq!(org.name, "Adobe");
    assert_eq!(
        org.legal_name.as_deref(),
        Some("Adobe Systems Incorporated")
    );
    assert_eq!(
        org.tagline.as_deref(),
        Some("Empowering everyone to create. ✨")
    );
    assert_eq!(org.founded_year, Some(1982));
    assert_eq!(org.headquarters.unwrap().city.as_deref(), Some("San Jose"));
    assert_eq!(org.revenue_range.unwrap().max_usd, None);
    let [round] = org.funding_rounds.as_slice() else {
        panic!("{:?}", org.funding_rounds);
    };
    assert_eq!(round.round_type, Some(FundingRoundType::SeriesUnknown));
    assert_eq!(round.announced_on, NaiveDate::from_ymd_opt(1984, 11, 1));
    assert_eq!(round.currency, Some(Currency::Usd));
    assert_eq!(round.investors[0].id, None);
    assert_eq!(round.investors[0].name, "Apple");
}

#[test]
fn a_person_reads_into_its_model() {
    let TypedEntity::Person(person) = entity(example(PERSON)).typed().unwrap() else {
        panic!("not a person");
    };
    assert_eq!(person.name, "Lisa Su");
    assert_eq!(person.follower_count, Some(239177));
    assert!(person.languages.is_empty());
    let job = &person.experience[0];
    assert_eq!(job.title.as_deref(), Some("Chair & CEO"));
    assert_eq!(job.start_date.as_deref(), Some("2022-02"));
    assert_eq!(job.end_date, None);
}

#[test]
fn a_place_reads_into_its_model() {
    let TypedEntity::Place(place) = entity(example(PLACE)).typed().unwrap() else {
        panic!("not a place");
    };
    assert_eq!(place.name, "Golden Gate Park");
    assert_eq!(place.opening_hours[0].day, Weekday::Monday);
    assert_eq!(place.status.state, PlaceState::Open);
    let rating = place.rating.unwrap();
    assert_eq!(rating.score, Some(4.8));
    assert_eq!(rating.distribution[4].count, 36276);
}

#[test]
fn a_crypto_asset_reads_into_its_model() {
    let TypedEntity::Crypto(coin) = entity(example(CRYPTO)).typed().unwrap() else {
        panic!("not a crypto asset");
    };
    assert_eq!(coin.symbol, "BTC");
    assert_eq!(coin.stale_hours, None);
    let latest = coin.latest.unwrap();
    assert_eq!(latest.low, 67500.0);
    assert_eq!(latest.change_pct, Some(-3.4976));
    let quote = coin.quote.unwrap();
    assert_eq!(quote.at, instant("2026-11-13T14:58:00Z"));
    assert_eq!(coin.chart.unwrap().prices.len(), 3);
    assert_eq!(coin.price_history, None);
}

#[test]
fn weather_reads_into_its_model() {
    let TypedEntity::Weather(weather) = entity(example(WEATHER)).typed().unwrap() else {
        panic!("not weather");
    };
    assert_eq!(weather.timezone, "America/New_York");
    let current = weather.current.unwrap();
    assert_eq!(current.temperature.c, 8.3);
    assert_eq!(current.condition.unwrap().label, "Partly cloudy");
    assert_eq!(
        weather.yesterday.unwrap().status,
        Some(WeatherStatus::Final)
    );
    let forecast = weather.forecast.unwrap();
    assert_eq!(forecast.issued_at, instant("2026-11-20T12:00:00Z"));
    assert_eq!(weather.days, None);
}

#[test]
fn a_type_this_version_does_not_model_keeps_its_properties() {
    let index = json!({
        "id": "https://world.epochal.com/index/spx",
        "type": "index",
        "schema_version": 1,
        "updated_at": "2026-11-20T21:00:00Z",
        "properties": {"name": "S&P 500", "level": 6012.5},
    });
    for sent in [index, example(STOCK)] {
        let kind = sent["type"].as_str().unwrap().to_owned();
        let read = entity(sent.clone());

        assert_eq!(
            read.typed().unwrap(),
            TypedEntity::Unknown {
                kind,
                schema_version: read.schema_version,
                properties: sent["properties"].clone(),
            }
        );
    }
}

#[test]
fn a_layout_newer_than_this_version_keeps_its_properties() {
    for text in [ORGANIZATION, PERSON, PLACE, CRYPTO, WEATHER] {
        let current = example(text);
        let mut newer = current.clone();
        newer["schema_version"] = json!(current["schema_version"].as_u64().unwrap() + 1);

        assert_eq!(
            entity(newer.clone()).typed().unwrap(),
            TypedEntity::Unknown {
                kind: newer["type"].as_str().unwrap().to_owned(),
                schema_version: newer["schema_version"].as_u64().unwrap() as u32,
                properties: newer["properties"].clone(),
            }
        );
        assert!(!matches!(
            entity(current).typed().unwrap(),
            TypedEntity::Unknown { .. }
        ));
    }
}

#[test]
fn an_enum_value_this_version_does_not_name_reads_as_other() {
    let mut sent = example(ORGANIZATION);
    sent["properties"]["funding_rounds"][0]["round_type"] = json!("series_z");
    sent["properties"]["funding_rounds"][0]["currency"] = json!("XAU");

    let TypedEntity::Organization(org) = entity(sent).typed().unwrap() else {
        panic!("not an organization");
    };
    let round = &org.funding_rounds[0];
    assert_eq!(
        round.round_type,
        Some(FundingRoundType::Other("series_z".into()))
    );
    assert_eq!(round.currency, Some(Currency::Other("XAU".into())));
}

#[test]
fn null_and_missing_fields_read_as_none() {
    let mut sent = example(ORGANIZATION);
    sent["properties"]["legal_name"] = Value::Null;
    sent["properties"]["headquarters"] = Value::Null;
    sent["properties"]["latest_funding_round"] = Value::Null;
    sent["properties"].as_object_mut().unwrap().remove("ticker");

    let TypedEntity::Organization(org) = entity(sent).typed().unwrap() else {
        panic!("not an organization");
    };
    assert_eq!(org.legal_name, None);
    assert_eq!(org.headquarters, None);
    assert_eq!(org.latest_funding_round, None);
    assert_eq!(org.ticker, None);
}

#[test]
fn an_instant_with_fractional_seconds_reads() {
    let mut sent = example(CRYPTO);
    sent["properties"]["quote"]["at"] = json!("2026-11-13T14:58:00.250Z");

    let TypedEntity::Crypto(coin) = entity(sent).typed().unwrap() else {
        panic!("not a crypto asset");
    };
    assert_eq!(coin.quote.unwrap().at, instant("2026-11-13T14:58:00.250Z"));
}

#[test]
fn properties_off_the_documented_layout_are_an_error() {
    let cases = [
        ("name", Value::Null),
        ("founded_year", json!("1993")),
        ("employee_count", json!(-1)),
        ("specialties", json!("ai")),
        ("funding_rounds", Value::Null),
        ("headquarters", json!("HQ")),
    ];
    for (field, value) in cases {
        let mut sent = example(ORGANIZATION);
        sent["properties"][field] = value;

        assert!(entity(sent).typed().is_err(), "{field}");
    }
}

#[test]
fn a_response_keeps_each_entitys_properties_as_sent() {
    let sent = json!({
        "as_of": "2026-11-21T00:00:00Z",
        "request_id": "req_9Zp4kL",
        "results": [],
        "entities": [example(ORGANIZATION), example(STOCK)],
    });

    let found: SearchResponse = serde_json::from_value(sent.clone()).unwrap();

    assert_eq!(
        found.entities[0].properties,
        sent["entities"][0]["properties"]
    );
    assert_eq!(
        found.entities[1].properties,
        sent["entities"][1]["properties"]
    );
}

#[test]
fn a_search_results_entity_reads_into_its_model() {
    let sent = example(ORGANIZATION);
    let found: SearchResponse = serde_json::from_value(json!({
        "as_of": "2026-11-21T00:00:00Z",
        "request_id": "req_9Zp4kL",
        "results": [{
            "title": "Adobe",
            "url": sent["id"],
            "snippet": "Adobe makes creative software.",
            "entity": sent,
        }],
        "entities": [],
    }))
    .unwrap();

    let entity = found.results[0].entity.as_ref().unwrap();
    assert_eq!(entity.properties, sent["properties"]);
    let TypedEntity::Organization(org) = entity.typed().unwrap() else {
        panic!("not an organization");
    };
    assert_eq!(org.name, "Adobe");
}
