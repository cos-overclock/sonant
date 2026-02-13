use std::time::Duration;

use crate::domain::LlmError;

pub(crate) fn read_env_var(name: &str) -> Result<Option<String>, LlmError> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(LlmError::validation(format!(
            "{name} could not be read: {error}"
        ))),
    }
}

pub(crate) fn parse_timeout_seconds(name: &str, value: &str) -> Result<Duration, LlmError> {
    let parsed = value.trim().parse::<u64>().map_err(|_| {
        LlmError::validation(format!("{name} must be a positive integer in seconds"))
    })?;
    if parsed == 0 {
        return Err(LlmError::validation(format!(
            "{name} must be greater than 0 seconds"
        )));
    }
    Ok(Duration::from_secs(parsed))
}

pub(crate) fn read_timeout_from_env(name: &str) -> Result<Option<Duration>, LlmError> {
    let Some(value) = read_env_var(name)? else {
        return Ok(None);
    };
    Ok(Some(parse_timeout_seconds(name, &value)?))
}

pub(crate) fn resolve_timeout_with_global_fallback<F>(
    provider_timeout: Option<Duration>,
    read_global_timeout: F,
    default_timeout: Duration,
) -> Result<Duration, LlmError>
where
    F: FnOnce() -> Result<Option<Duration>, LlmError>,
{
    if let Some(timeout) = provider_timeout {
        return Ok(timeout);
    }

    Ok(read_global_timeout()?.unwrap_or(default_timeout))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::time::Duration;

    use crate::domain::LlmError;

    use super::{parse_timeout_seconds, resolve_timeout_with_global_fallback};

    #[test]
    fn parse_timeout_seconds_accepts_positive_integer_values() {
        let timeout = parse_timeout_seconds("TEST_TIMEOUT", "8")
            .expect("positive integer timeout should parse");
        assert_eq!(timeout, Duration::from_secs(8));
    }

    #[test]
    fn parse_timeout_seconds_rejects_invalid_values() {
        let zero = parse_timeout_seconds("TEST_TIMEOUT", "0")
            .expect_err("zero timeout should fail validation");
        assert!(matches!(
            zero,
            LlmError::Validation { message }
            if message == "TEST_TIMEOUT must be greater than 0 seconds"
        ));

        let invalid = parse_timeout_seconds("TEST_TIMEOUT", "abc")
            .expect_err("non-integer timeout should fail validation");
        assert!(matches!(
            invalid,
            LlmError::Validation { message }
            if message == "TEST_TIMEOUT must be a positive integer in seconds"
        ));
    }

    #[test]
    fn resolve_timeout_with_global_fallback_is_lazy_for_provider_timeout() {
        let global_called = Cell::new(false);

        let timeout = resolve_timeout_with_global_fallback(
            Some(Duration::from_secs(3)),
            || {
                global_called.set(true);
                Err(LlmError::validation("global timeout should not be parsed"))
            },
            Duration::from_secs(8),
        )
        .expect("provider-specific timeout should short-circuit global fallback");

        assert_eq!(timeout, Duration::from_secs(3));
        assert!(!global_called.get());
    }

    #[test]
    fn resolve_timeout_with_global_fallback_uses_global_when_provider_absent() {
        let timeout = resolve_timeout_with_global_fallback(
            None,
            || Ok(Some(Duration::from_secs(9))),
            Duration::from_secs(8),
        )
        .expect("global timeout should be used when provider timeout is absent");

        assert_eq!(timeout, Duration::from_secs(9));
    }

    #[test]
    fn resolve_timeout_with_global_fallback_uses_default_when_missing() {
        let timeout =
            resolve_timeout_with_global_fallback(None, || Ok(None), Duration::from_secs(8))
                .expect("default timeout should be used when both env vars are missing");

        assert_eq!(timeout, Duration::from_secs(8));
    }
}
