use serde_json::json;

#[derive(thiserror::Error, Debug)]
pub enum AddToPathError {
    #[error("Value for field {0} must be an object")]
    ObjectRequired(serde_json::Value),
}

pub(crate) fn set_field_path(
    root: &mut serde_json::Value,
    path: &str,
    leaf: serde_json::Value,
) -> std::result::Result<(), AddToPathError> {
    use serde_json::Value::Object;

    if let Object(map) = root {
        let mut map = map;
        let mut path = path;
        loop {
            match path.split_once('.') {
                // this is the leaf field, just add it to the map and we're done.
                None => {
                    map.insert(path.to_string(), leaf);
                    return Ok(());
                }
                // otherwise, we have to create an object that will hold the leaf or a subtree, and iterate on.
                Some((field, rest)) => {
                    map = match map.entry(field).or_insert(json!({})) {
                        Object(inner_map) => inner_map,
                        _ => return Err(AddToPathError::ObjectRequired(root.clone())),
                    };
                    path = rest;
                }
            };
        }
    } else {
        Err(AddToPathError::ObjectRequired(root.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rstest::*;

    #[rstest]
    #[case("status", r#"{"spec":{},"status":"demo"}"#)]
    #[case("status.foo", r#"{"spec":{},"status":{"keep":1,"foo":"demo"}}"#)]
    #[case(
        "status.foo.bar",
        r#"{"spec":{},"status":{"keep":1,"foo":{"bar":"demo"}}}"#
    )]
    fn test_add_to_path(#[case] path: &str, #[case] expected: &str) {
        let mut root = json!({ "spec": {}, "status": {"keep": 1} });
        set_field_path(
            &mut root,
            path,
            serde_json::Value::String("demo".to_string()),
        )
        .unwrap();

        assert_eq!(serde_json::to_string(&root).unwrap(), expected);
    }
}
