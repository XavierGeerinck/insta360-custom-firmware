#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MeetingStatus {
    Recording,
    Finalized,
    AwaitingDecision,
    LocalOnly,
    Queued,
    Uploading,
    RetryWait,
    Uploaded,
    ConfigurationError,
    MissingArtifact,
    LegacyUnsupported,
    ArtifactError,
}

impl MeetingStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recording => "recording",
            Self::Finalized => "finalized",
            Self::AwaitingDecision => "awaiting_decision",
            Self::LocalOnly => "local_only",
            Self::Queued => "queued",
            Self::Uploading => "uploading",
            Self::RetryWait => "retry_wait",
            Self::Uploaded => "uploaded",
            Self::ConfigurationError => "configuration_error",
            Self::MissingArtifact => "missing_artifact",
            Self::LegacyUnsupported => "legacy_unsupported",
            Self::ArtifactError => "artifact_error",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "recording" => Self::Recording,
            "finalized" => Self::Finalized,
            "awaiting_decision" => Self::AwaitingDecision,
            "local_only" => Self::LocalOnly,
            "queued" | "pending" => Self::Queued,
            "uploading" => Self::Uploading,
            "retry_wait" | "upload_failed" => Self::RetryWait,
            "uploaded" => Self::Uploaded,
            "configuration_error" => Self::ConfigurationError,
            "missing_artifact" => Self::MissingArtifact,
            "legacy_unsupported" => Self::LegacyUnsupported,
            "artifact_error" => Self::ArtifactError,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_statuses_parse_without_becoming_uploadable_by_themselves() {
        assert_eq!(
            MeetingStatus::parse("upload_failed"),
            Some(MeetingStatus::RetryWait)
        );
        assert_eq!(MeetingStatus::parse("pending"), Some(MeetingStatus::Queued));
        assert_eq!(MeetingStatus::parse("unknown"), None);
    }
}
