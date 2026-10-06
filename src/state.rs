use std::{
    any::{Any, TypeId},
    collections::HashMap,
    sync::Arc,
};

/// App-wide shared values, one per type. See [`App::state`](crate::App::state).
#[derive(Clone, Default)]
pub(crate) struct State(HashMap<TypeId, Arc<dyn Any + Send + Sync>>);

impl State {
    pub(crate) fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
        self.0.insert(TypeId::of::<T>(), Arc::new(value));
    }

    pub(crate) fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.0.get(&TypeId::of::<T>())?.downcast_ref()
    }
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("values", &self.0.len())
            .finish()
    }
}
