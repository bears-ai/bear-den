//! Opaque native runtime handles derived from verified conversation or Job-run sources.
//! A handle identifies the source of a turn; possession of its text never grants access.

use den_core::ids::BearId;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeTurnSource {
    Conversation(Uuid),
    WorkRun(Uuid),
}

impl NativeTurnSource {
    pub fn binding_id(self, bear_id: BearId) -> String {
        match self {
            Self::Conversation(id) => format!("den-native:{}:conversation:{id}", bear_id.as_uuid()),
            Self::WorkRun(id) => format!("den-native:{}:work-run:{id}", bear_id.as_uuid()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distinct_sources_and_bears_cannot_share_a_binding() {
        let bear = BearId::new(Uuid::new_v4());
        let other = BearId::new(Uuid::new_v4());
        let id = Uuid::new_v4();
        let conversation = NativeTurnSource::Conversation(id).binding_id(bear);
        assert_ne!(conversation, NativeTurnSource::WorkRun(id).binding_id(bear));
        assert_ne!(
            conversation,
            NativeTurnSource::Conversation(id).binding_id(other)
        );
        assert_ne!(
            conversation,
            NativeTurnSource::Conversation(Uuid::new_v4()).binding_id(bear)
        );
    }
}
