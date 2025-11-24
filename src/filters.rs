use kube::Resource;

pub trait Filterable {
    fn was_last_modified_by(&self, manager: &str) -> Option<bool>;
}

impl<D, S, K> Filterable for K
where
    K: Resource<DynamicType = D, Scope = S>,
{
    fn was_last_modified_by(&self, manager: &str) -> Option<bool> {
        match self.meta().managed_fields.as_ref() {
            None => None,
            Some(managed_fields) if managed_fields.iter().any(|field| field.time.is_none()) => None,
            Some(managed_fields) => {
                let most_recent_entry = managed_fields
                    .iter()
                    .max_by_key(|&field| field.time.as_ref());
                most_recent_entry.map(|field| field.manager == Some(manager.to_string()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::iter;

    use chrono::TimeZone;
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ManagedFieldsEntry, ObjectMeta, Time};
    use once_cell::sync::Lazy;
    use rand::distr::Alphanumeric;
    use rand::Rng;
    use rstest::*;

    use super::*;

    macro_rules! rand_string {
        ($len:expr) => {
            Lazy::new(|| {
                iter::repeat(())
                    .map(|()| rand::rng().sample::<u8, _>(Alphanumeric))
                    .filter(|c| c.is_ascii_alphabetic())
                    .take($len)
                    .map(char::from)
                    .collect()
            })
        };
        () => {
            rand_string!(10)
        };
    }

    static MANAGER: Lazy<String> = rand_string!();
    static OTHER_MANAGER: Lazy<String> = rand_string!();
    static NOW: Lazy<Time> = Lazy::new(|| Time(chrono::Utc::now()));
    static EPOCH: Lazy<Time> = Lazy::new(|| Time(chrono::Utc.timestamp_opt(0, 0).unwrap()));

    #[rstest]
    #[case::no_managed_fields(None, None)]
    #[case::empty_managed_fields(Some(vec ! []), None)]
    #[
        case::most_recent_entry_matches_manager(
            Some(
                vec ! [
                ManagedFieldsEntry
                {
                manager: Some(MANAGER.clone()),
                time: Some(NOW.clone()),
                ..Default::default()
                }, ManagedFieldsEntry
                {
                manager: Some(OTHER_MANAGER.clone()),
                time: Some(EPOCH.clone()),
                ..Default::default()
                }
                ]
            ),
            Some(true)
        )
    ]
    #[
        case::most_recent_entry_does_not_match_manager(
            Some(
                vec ! [
                ManagedFieldsEntry
                {
                manager: Some(OTHER_MANAGER.clone()),
                time: Some(NOW.clone()),
                ..Default::default()
                }, ManagedFieldsEntry
                {
                manager: Some(MANAGER.clone()),
                time: Some(EPOCH.clone()),
                ..Default::default()
                }
                ]
            ),
            Some(false)
        )
    ]
    #[
        case::one_or_more_entries_has_no_timestamp(
            Some(
                vec ! [
                ManagedFieldsEntry
                {
                manager: Some(MANAGER.clone()),
                time: Some(NOW.clone()),
                ..Default::default()
                }, ManagedFieldsEntry
                {
                manager: Some(OTHER_MANAGER.clone()),
                time: None,
                ..Default::default()
                }
                ]
            ),
            None,
        )
    ]
    #[tokio::test]
    async fn test_was_last_modified_by(
        #[case] managed_fields: Option<Vec<ManagedFieldsEntry>>,
        #[case] expected: Option<bool>,
    ) {
        let resource = k8s_openapi::api::core::v1::Pod {
            metadata: ObjectMeta {
                managed_fields,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(resource.was_last_modified_by(&MANAGER), expected);
    }
}
