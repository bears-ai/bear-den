use super::Feedback;

#[test]
fn one_time_feedback_requires_the_issuing_operator_and_a_live_expiry() {
    let feedback = || Feedback {
        operator_id: 7,
        expires_at: 100,
        value: "private credential",
    };
    assert_eq!(feedback().consume_for(7, 99), Some("private credential"));
    assert_eq!(feedback().consume_for(8, 99), None);
    assert_eq!(feedback().consume_for(7, 100), None);
    assert_eq!(feedback().consume_for(7, 101), None);
}
