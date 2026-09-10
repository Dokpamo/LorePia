use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportResourcePolicyDto {
    Standard,
    UserApprovedLarge,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PickImportRequest {
    pub resource_policy: ImportResourcePolicyDto,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ImportTicketDto {
    pub ticket_id: String,
    pub display_name: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TicketRequest {
    pub ticket_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InspectionRequest {
    pub inspection_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DiscardImportRequest {
    Ticket { ticket_id: String },
    Inspection { inspection_id: String },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ImportResourcePolicyDto, PickImportRequest};

    #[test]
    fn import_resource_policy_accepts_only_the_two_fixed_envelopes() {
        for (wire, expected) in [
            ("standard", ImportResourcePolicyDto::Standard),
            (
                "user_approved_large",
                ImportResourcePolicyDto::UserApprovedLarge,
            ),
        ] {
            let request: PickImportRequest = serde_json::from_value(json!({
                "resource_policy": wire
            }))
            .expect("fixed resource policy");
            assert_eq!(request.resource_policy, expected);
        }
        assert!(
            serde_json::from_value::<PickImportRequest>(json!({
                "resource_policy": "unlimited"
            }))
            .is_err()
        );
    }
}
