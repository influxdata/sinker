use kube::runtime::reflector::ObjectRef;
use kube::Resource;
use std::collections::HashMap;

use crate::resources::ResourceSync;

/// Retain generation history by name and namespace for the lifetime of the watch.
/// Kube's predicate filter now expires entries and keys them by UID as well.
pub(crate) fn generation_changed() -> impl FnMut(&ResourceSync) -> bool {
    let mut generations = HashMap::new();
    move |resource| match resource.metadata.generation {
        Some(generation) => {
            generations.insert(ObjectRef::from_obj(resource), generation) != Some(generation)
        }
        None => true,
    }
}

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

    #[test]
    fn generation_filter_preserves_history_across_metadata_and_uid_changes() {
        let mut changed = generation_changed();
        let mut resource = crate::test_support::resource_sync();
        resource.metadata.generation = Some(1);
        assert!(changed(&resource));
        assert!(!changed(&resource));

        resource.metadata.resource_version = Some("2".into());
        resource.metadata.annotations = Some([("updated".into(), "true".into())].into());
        resource.status = Some(crate::resources::ResourceSyncStatus::default());
        assert!(!changed(&resource));
        resource.metadata.uid = Some("replacement-uid".into());
        assert!(!changed(&resource));

        resource.metadata.generation = None;
        assert!(changed(&resource));
        assert!(changed(&resource));
        resource.metadata.generation = Some(1);
        assert!(
            !changed(&resource),
            "missing generations do not erase history"
        );
        resource.metadata.generation = Some(2);
        assert!(changed(&resource));
        assert!(!changed(&resource));
        resource.metadata.generation = Some(1);
        assert!(changed(&resource), "any generation change is emitted");
    }

    #[rstest]
    #[case::different_name("other", "default")]
    #[case::different_namespace("copy-config", "other")]
    fn generation_filter_tracks_names_and_namespaces_independently(
        #[case] name: &str,
        #[case] namespace: &str,
    ) {
        let mut changed = generation_changed();
        let mut original = crate::test_support::resource_sync();
        original.metadata.name = Some("copy-config".into());
        original.metadata.namespace = Some("default".into());
        original.metadata.generation = Some(1);
        let mut other = original.clone();
        other.metadata.name = Some(name.into());
        other.metadata.namespace = Some(namespace.into());

        assert!(changed(&original));
        assert!(changed(&other));
        assert!(!changed(&original));
        assert!(!changed(&other));
    }

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
