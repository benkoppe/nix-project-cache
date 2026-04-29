use std::collections::BTreeMap;

use serde_json::{Map, Value};
use thiserror::Error;
use wildmatch::WildMatch;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum OidcClaimsError {
    #[error("required claim {0:?} not found")]
    MissingClaim(String),
    #[error("claim {claim:?} value {values:?} not in allowed patterns {allowed_patterns:?}")]
    ClaimMismatch {
        claim: String,
        values: Vec<String>,
        allowed_patterns: Vec<String>,
    },
    #[error("subject {subject:?} not in allowed patterns {allowed_patterns:?}")]
    SubjectMismatch {
        subject: String,
        allowed_patterns: Vec<String>,
    },
}

pub fn validate_bound_claims(
    claims: &Map<String, Value>,
    bound_claims: &BTreeMap<String, Vec<String>>,
) -> Result<(), OidcClaimsError> {
    for (claim_name, allowed_patterns) in bound_claims {
        let value = get_claim(claims, claim_name)
            .ok_or_else(|| OidcClaimsError::MissingClaim(claim_name.clone()))?;

        let values = normalize_to_string_vec(value);
        if values.is_empty() {
            return Err(OidcClaimsError::MissingClaim(claim_name.clone()));
        }

        let matched = values.iter().any(|value| {
            allowed_patterns
                .iter()
                .any(|pattern| WildMatch::new(pattern).matches(value))
        });

        if !matched {
            return Err(OidcClaimsError::ClaimMismatch {
                claim: claim_name.clone(),
                values,
                allowed_patterns: allowed_patterns.clone(),
            });
        }
    }

    Ok(())
}

pub fn validate_bound_subject(
    claims: &Map<String, Value>,
    bound_subject: &[String],
) -> Result<(), OidcClaimsError> {
    if bound_subject.is_empty() {
        return Ok(());
    }

    let subject = get_claim(claims, "sub")
        .and_then(Value::as_str)
        .ok_or_else(|| OidcClaimsError::MissingClaim("sub".to_owned()))?;

    if bound_subject
        .iter()
        .any(|pattern| WildMatch::new(pattern).matches(subject))
    {
        Ok(())
    } else {
        Err(OidcClaimsError::SubjectMismatch {
            subject: subject.to_owned(),
            allowed_patterns: bound_subject.to_vec(),
        })
    }
}

pub fn get_claim<'a>(claims: &'a Map<String, Value>, name: &str) -> Option<&'a Value> {
    if let Some(value) = claims.get(name) {
        return Some(value);
    }

    let parts = name.split('.').collect::<Vec<_>>();
    if parts.len() == 1 {
        return None;
    }

    let mut current = claims.get(parts[0])?;
    for part in &parts[1..] {
        current = current.as_object()?.get(*part)?;
    }

    Some(current)
}

pub fn get_string_claim(claims: &Map<String, Value>, name: &str) -> Option<String> {
    get_claim(claims, name)
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn normalize_to_string_vec(value: &Value) -> Vec<String> {
    match value {
        Value::String(value) => vec![value.clone()],
        Value::Array(values) => values
            .iter()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
        Value::Null => Vec::new(),
        other => vec![other.to_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_bound_claims_matches_string_and_nested_claims() {
        let claims = serde_json::json!({
            "sub": "repo:owner/repo:ref:refs/heads/main",
            "repository": "owner/repo",
            "github": {
                "ref": "refs/heads/main"
            }
        })
        .as_object()
        .unwrap()
        .clone();

        let bound = BTreeMap::from([
            ("repository".to_owned(), vec!["owner/*".to_owned()]),
            ("github.ref".to_owned(), vec!["refs/heads/*".to_owned()]),
        ]);

        validate_bound_claims(&claims, &bound).unwrap();
    }

    #[test]
    fn validate_bound_subject_matches_glob() {
        let claims = serde_json::json!({
            "sub": "repo:owner/repo:ref:refs/heads/main"
        })
        .as_object()
        .unwrap()
        .clone();

        validate_bound_subject(&claims, &[String::from("repo:owner/*:*")]).unwrap();
    }
}
