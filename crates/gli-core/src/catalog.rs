//! Registry of available modules. A plain factory, no DI container.

use crate::{Category, Module};

/// Owns the enabled modules and exposes lookup, iteration, and filtering.
/// Modules are kept sorted by category rank so security comes first.
pub struct ModuleCatalog {
    modules: Vec<Box<dyn Module>>,
}

impl ModuleCatalog {
    /// Build a catalog, sorting modules by category (security baseline first).
    pub fn new(mut modules: Vec<Box<dyn Module>>) -> Self {
        modules.sort_by_key(|m| m.category().rank());
        Self { modules }
    }

    /// Look up a module by its stable id.
    pub fn get(&self, name: &str) -> Option<&dyn Module> {
        self.iter().find(|m| m.name() == name)
    }

    /// Iterate every module in plan order.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Module> {
        self.modules.iter().map(AsRef::as_ref)
    }

    /// Iterate only modules in `category`.
    pub fn by_category(&self, category: Category) -> impl Iterator<Item = &dyn Module> {
        self.iter().filter(move |m| m.category() == category)
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }
}
