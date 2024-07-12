#[macro_export]
macro_rules! requeue_after {
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
