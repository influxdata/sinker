#[macro_export]
macro_rules! requeue_after {
    (in_seconds $s:expr) => {
        Ok(Action::requeue(Duration::from_secs($s)))
    };
    ($duration:expr) => {
        Ok(Action::requeue($duration))
    };
    () => {
        Ok(Action::requeue(Duration::from_secs(5)))
    };
}

pub trait WithItemRemoved<T> {
    fn with_item_removed(self, item: &T) -> Self;
}

impl<T> WithItemRemoved<T> for Vec<T>
where
    T: PartialEq,
{
    fn with_item_removed(mut self, item: &T) -> Self {
        self.retain(|i| i != item);
        self
    }
}

pub trait WithItemAdded<T> {
    fn with_push(self, item: T) -> Self;
}

impl<T> WithItemAdded<T> for Vec<T> {
    fn with_push(mut self, item: T) -> Self {
        self.push(item);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::runtime::controller::Action;
    use rstest::rstest;
    use std::time::Duration;

    #[rstest]
    #[case::empty(vec![], vec![])]
    #[case::absent(vec!["first", "last"], vec!["first", "last"])]
    #[case::duplicates(vec!["remove", "first", "remove", "last"], vec!["first", "last"])]
    #[case::only_match(vec!["remove"], vec![])]
    fn removing_items_preserves_other_items_and_order(
        #[case] input: Vec<&str>,
        #[case] expected: Vec<&str>,
    ) {
        assert_eq!(input.with_item_removed(&"remove"), expected);
    }

    #[test]
    fn with_push_appends_without_deduplicating() {
        assert_eq!(
            Vec::new()
                .with_push("first")
                .with_push("second")
                .with_push("first"),
            vec!["first", "second", "first"]
        );
    }

    #[test]
    fn requeue_macro_supports_default_seconds_and_subsecond_durations() {
        let default: crate::Result<Action> = requeue_after!();
        let seconds: crate::Result<Action> = requeue_after!(in_seconds 12);
        let duration: crate::Result<Action> = requeue_after!(Duration::from_millis(500));
        assert_eq!(
            default.expect("default requeue"),
            Action::requeue(Duration::from_secs(5))
        );
        assert_eq!(
            seconds.expect("seconds requeue"),
            Action::requeue(Duration::from_secs(12))
        );
        assert_eq!(
            duration.expect("duration requeue"),
            Action::requeue(Duration::from_millis(500))
        );
    }
}
