pub mod code_host;
pub mod container_runtime;
pub mod event_source;
pub mod session_store;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::*;

    #[test]
    fn event_source_is_object_safe() {
        fn _assert(_: Box<dyn event_source::EventSource>) {}
    }

    #[test]
    fn container_runtime_is_object_safe() {
        fn _assert(_: Box<dyn container_runtime::ContainerRuntime>) {}
    }

    #[test]
    fn session_store_is_object_safe() {
        fn _assert(_: Box<dyn session_store::SessionStore>) {}
    }

    #[test]
    fn code_host_is_object_safe() {
        fn _assert(_: Box<dyn code_host::CodeHost>) {}
    }
}
