pub(super) fn open_replacement<T, U, E>(
    current: &mut Option<T>,
    mut open: impl FnMut() -> Result<U, E>,
    release: impl FnOnce(T),
    retry_after_release: impl FnOnce(&E) -> bool,
) -> Result<U, E> {
    match open() {
        Err(error) if current.is_some() && retry_after_release(&error) => {
            release(current.take().expect("current stream checked above"));
            open()
        }
        result => result,
    }
}
