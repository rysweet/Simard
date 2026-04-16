use proptest::prelude::*;
use simard::{
    GoalStatus, ImprovementPromotionPlan, MeetingCommand, PersistedImprovementRecord,
    PersistedMeetingRecord, SessionId, parse_copilot_response, parse_meeting_command,
    parse_turn_output,
};

proptest! {
    #[test]
    fn session_id_parse_never_panics(s in "\\PC*") {
        let _ = SessionId::parse(&s);
    }

    #[test]
    fn parse_meeting_command_never_panics(s in "\\PC*") {
        // Returns MeetingCommand (infallible), just must not panic.
        let cmd = parse_meeting_command(&s);
        // exhaustive match — just verify no panic
        let _ = cmd;
    }

    #[test]
    fn parse_turn_output_never_panics(s in "\\PC*") {
        let _ = parse_turn_output(&s);
    }

    #[test]
    fn parse_copilot_response_never_panics(s in "\\PC*") {
        let _ = parse_copilot_response(&s);
    }

    #[test]
    fn goal_status_parse_never_panics(s in "\\PC*") {
        let _ = GoalStatus::parse(&s);
    }

    #[test]
    fn persisted_meeting_record_parse_never_panics(s in "\\PC*") {
        let _ = PersistedMeetingRecord::parse(&s);
    }

    #[test]
    fn persisted_improvement_record_parse_never_panics(s in "\\PC*") {
        let _ = PersistedImprovementRecord::parse(&s);
    }

    #[test]
    fn improvement_promotion_plan_parse_never_panics(s in "\\PC*") {
        let _ = ImprovementPromotionPlan::parse(&s);
    }
}
