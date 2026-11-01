use crate::delivery::{Delivery, ProtocolConfirmation};
use serde_json::Value;

pub(crate) struct TurnCorrelation {
    session: String,
    uuid: String,
    confirmation: ProtocolConfirmation,
    replayed: bool,
    started: bool,
    pub completed: bool,
}
impl TurnCorrelation {
    pub fn new(session: String, uuid: String) -> Self {
        Self {
            confirmation: ProtocolConfirmation::new(session.clone(), uuid.clone()),
            session,
            uuid,
            replayed: false,
            started: false,
            completed: false,
        }
    }
    pub fn observe(&mut self, value: &Value) {
        if value["session_id"].as_str() != Some(self.session.as_str())
            || !value["parent_tool_use_id"].is_null()
            || !value["agent_id"].is_null()
        {
            return;
        }
        if value["type"] == "user"
            && value["isReplay"] == true
            && value["uuid"].as_str() == Some(self.uuid.as_str())
        {
            self.replayed = true;
        }
        if self.replayed
            && value["type"] == "stream_event"
            && value["event"]["type"] == "message_start"
        {
            self.started = true;
        }
        if self.replayed && self.started {
            self.confirmation.observe_started(&self.session, &self.uuid);
        }
        if value["type"] == "result"
            && value["user_message_uuids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(self.uuid.as_str())))
        {
            self.completed = true;
        }
    }
    pub fn delivery(&self) -> Delivery {
        self.confirmation.delivery()
    }
    pub fn uuid(&self) -> &str {
        &self.uuid
    }
}
