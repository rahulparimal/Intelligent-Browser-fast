use std::collections::HashMap;
use std::sync::Arc;

/// Interned string handle: 4 bytes instead of a heap string per occurrence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sym(u32);

/// String interner for tag, role and attribute names, which repeat thousands of times per page.
#[derive(Debug, Default)]
pub struct Interner {
    map: HashMap<Arc<str>, Sym>,
    strings: Vec<Arc<str>>,
}

impl Interner {
    pub fn new() -> Self {
        Self::default()
    }

    /// # Panics
    /// If more than `u32::MAX` distinct strings are interned.
    pub fn intern(&mut self, s: &str) -> Sym {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = Sym(u32::try_from(self.strings.len()).expect("interner overflow"));
        let arc: Arc<str> = Arc::from(s);
        self.strings.push(Arc::clone(&arc));
        self.map.insert(arc, sym);
        sym
    }

    pub fn resolve(&self, sym: Sym) -> &str {
        &self.strings[sym.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_string_same_symbol_and_roundtrip() {
        let mut i = Interner::new();
        let a = i.intern("button");
        let b = i.intern("link");
        assert_eq!(a, i.intern("button"));
        assert_ne!(a, b);
        assert_eq!((i.resolve(a), i.resolve(b), i.len()), ("button", "link", 2));
        assert!(!i.is_empty());
    }
}
