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
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ManagedFieldsEntry, ObjectMeta, Time};
    use k8s_openapi::jiff::Timestamp;
    use once_cell::sync::Lazy;
    use rstest::*;

    use super::*;

    static MANAGER: Lazy<String> = Lazy::new(|| "sinker.influxdata.io".into());
    static OTHER_MANAGER: Lazy<String> = Lazy::new(|| "external-manager".into());
    static NOW: Lazy<Time> =
        Lazy::new(|| Time(Timestamp::from_second(1_700_000_000).expect("fixed timestamp")));
    static EPOCH: Lazy<Time> = Lazy::new(|| Time(Timestamp::UNIX_EPOCH));

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

    #[rstest]
    #[case::unknown_manager(None, Some(false))]
    #[case::sinker_manager(Some("sinker.influxdata.io"), Some(true))]
    #[case::external_manager(Some("external"), Some(false))]
    fn latest_entry_wins_regardless_of_position(
        #[case] manager: Option<&str>,
        #[case] expected: Option<bool>,
    ) {
        let latest = NOW.clone();
        let old = ManagedFieldsEntry {
            manager: Some("sinker.influxdata.io".into()),
            time: Some(EPOCH.clone()),
            ..Default::default()
        };
        let current = ManagedFieldsEntry {
            manager: manager.map(String::from),
            time: Some(latest),
            ..Default::default()
        };
        for entries in [vec![old.clone(), current.clone()], vec![current, old]] {
            let mut resource = crate::test_support::resource_sync();
            resource.metadata.managed_fields = Some(entries);
            assert_eq!(
                resource.was_last_modified_by("sinker.influxdata.io"),
                expected
            );
        }
    }
}
