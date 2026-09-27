use super::*;

#[test]
fn bear_id_round_trips_through_uuid_and_string() {
    let raw = Uuid::nil();
    let id = BearId::from(raw);
    assert_eq!(id.as_uuid(), raw);
    assert_eq!(Uuid::from(id), raw);
    assert_eq!(id.to_string(), raw.to_string());
    assert_eq!(BearId::from_str(&raw.to_string()).unwrap(), id);
}

#[test]
fn bear_id_serde_is_transparent() {
    let id = BearId::new(Uuid::nil());
    let json = serde_json::to_string(&id).unwrap();
    assert_eq!(json, format!("\"{}\"", Uuid::nil()));
    let back: BearId = serde_json::from_str(&json).unwrap();
    assert_eq!(back, id);
}

#[test]
fn hat_id_is_distinct_and_round_trips() {
    let raw = Uuid::new_v4();
    let hat = HatId::new(raw);
    assert_eq!(hat.as_uuid(), raw);
    assert_eq!(Uuid::from(hat), raw);
    assert_eq!(HatId::from_str(&raw.to_string()).unwrap(), hat);
    let json = serde_json::to_string(&hat).unwrap();
    assert_eq!(serde_json::from_str::<HatId>(&json).unwrap(), hat);
}

#[test]
fn user_id_orders_and_converts() {
    assert!(UserId::new(1) < UserId::new(2));
    assert_eq!(i32::from(UserId::from(7)), 7);
    assert_eq!(UserId::new(7).get(), 7);
}

#[test]
fn string_ids_are_transparent() {
    let s = SessionId::new("client-123");
    assert_eq!(s.as_str(), "client-123");
    assert_eq!(serde_json::to_string(&s).unwrap(), "\"client-123\"");

    let c = ConversationId::from("default");
    assert_eq!(c.to_string(), "default");
    assert_eq!(c.into_string(), "default");
}
