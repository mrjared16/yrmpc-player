pub mod paths;
pub mod resilient;

use serde_json::Value;

pub fn extract_string(json: &Value, paths: &[&str]) -> Option<String> {
    for path in paths {
        if let Some(value) = extract_at_path(json, path) {
            if let Some(s) = value.as_str() {
                return Some(s.to_string());
            }
        }
    }
    None
}

pub fn extract_array<'a>(json: &'a Value, paths: &[&str]) -> Option<&'a Vec<Value>> {
    for path in paths {
        if let Some(value) = extract_at_path(json, path) {
            if let Some(arr) = value.as_array() {
                return Some(arr);
            }
        }
    }
    None
}

fn extract_at_path<'a>(json: &'a Value, path: &str) -> Option<&'a Value> {
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let mut current = json;

    for segment in segments {
        if segment.is_empty() {
            continue;
        }

        if let Some(obj) = current.as_object() {
            current = obj.get(segment)?;
            continue;
        }

        if let Ok(index) = segment.parse::<usize>() {
            if let Some(arr) = current.as_array() {
                current = arr.get(index)?;
                continue;
            }
        }

        return None;
    }

    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_extract_string_first_path() {
        let json = json!({
            "onTap": {
                "watchEndpoint": {
                    "videoId": "abc123"
                }
            }
        });

        let result = extract_string(&json, paths::video_id::ALL);
        assert_eq!(result, Some("abc123".into()));
    }

    #[test]
    fn test_extract_string_fallback() {
        let json = json!({
            "title": {
                "runs": [{
                    "navigationEndpoint": {
                        "watchEndpoint": {
                            "videoId": "fallback123"
                        }
                    }
                }]
            }
        });

        let result = extract_string(&json, paths::video_id::ALL);
        assert_eq!(result, Some("fallback123".into()));
    }

    #[test]
    fn test_extract_string_none() {
        let json = json!({
            "unrelated": "data"
        });

        let result = extract_string(&json, paths::video_id::ALL);
        assert_eq!(result, None);
    }
}
