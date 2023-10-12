pub enum Cancellable<T> {
    Cancelled,
    Accepted(T),
}
