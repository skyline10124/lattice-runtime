pub use lattice_core::behavior::{BehaviorMode, YoloSandboxPolicy};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_behavior_is_strict() {
        let b = BehaviorMode::default();
        assert!(b.is_strict());
        assert!(!b.is_yolo());
    }

    #[test]
    fn test_yolo_behavior() {
        let b = BehaviorMode::Yolo {
            enforce_sandbox: YoloSandboxPolicy::EnforceCommandAllowlist,
        };
        assert!(b.is_yolo());
        assert!(!b.is_strict());
    }

    #[test]
    fn test_behavior_mode_clone() {
        let b = BehaviorMode::Strict {
            confidence_threshold: 0.9,
            max_retries: 5,
            escalate_to: Some("lead".into()),
        };
        let b2 = b.clone();
        assert_eq!(b, b2);
    }
}
