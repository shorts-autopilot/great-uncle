//! Handle registry for objects that are not user-numbered (fixture types now,
//! pools and executors later).
//!
//! Handles carry a generation so a handle to a removed object can never resolve
//! to its replacement. Fixtures are addressed by their fixture number instead:
//! that number is the user-facing identity and is part of the show file.

use std::collections::BTreeMap;

/// Opaque, generation-checked reference to a registered object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Handle {
    index: u32,
    generation: u16,
}

impl Handle {
    /// Stable value for logging and UI keys (not stable across reloads).
    pub fn id(self) -> u64 {
        (u64::from(self.generation) << 32) | u64::from(self.index)
    }
}

struct Slot<T> {
    generation: u16,
    value: Option<T>,
}

pub struct Registry<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Default for Registry<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Registry<T> {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn insert(&mut self, value: T) -> Handle {
        let index = match self.free.pop() {
            Some(index) => index,
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    value: None,
                });
                (self.slots.len() - 1) as u32
            }
        };
        let slot = &mut self.slots[index as usize];
        slot.value = Some(value);
        self.len += 1;
        Handle {
            index,
            generation: slot.generation,
        }
    }

    pub fn get(&self, handle: Handle) -> Option<&T> {
        let slot = self.slots.get(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_ref()
    }

    pub fn get_mut(&mut self, handle: Handle) -> Option<&mut T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        slot.value.as_mut()
    }

    pub fn contains(&self, handle: Handle) -> bool {
        self.get(handle).is_some()
    }

    pub fn remove(&mut self, handle: Handle) -> Option<T> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        if slot.generation != handle.generation {
            return None;
        }
        let value = slot.value.take()?;
        // Bump the generation so the old handle stays invalid after reuse.
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(handle.index);
        self.len -= 1;
        Some(value)
    }

    pub fn iter(&self) -> impl Iterator<Item = (Handle, &T)> {
        self.slots.iter().enumerate().filter_map(|(index, slot)| {
            slot.value.as_ref().map(|value| {
                (
                    Handle {
                        index: index as u32,
                        generation: slot.generation,
                    },
                    value,
                )
            })
        })
    }

    pub fn clear(&mut self) {
        for (index, slot) in self.slots.iter_mut().enumerate() {
            if slot.value.take().is_some() {
                slot.generation = slot.generation.wrapping_add(1);
                self.free.push(index as u32);
            }
        }
        self.len = 0;
    }
}

/// Lookup by a case-insensitive name key, on top of a [`Registry`].
pub struct NameIndex<T> {
    handles: BTreeMap<String, Handle>,
    registry: Registry<T>,
}

impl<T> Default for NameIndex<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> NameIndex<T> {
    pub fn new() -> Self {
        Self {
            handles: BTreeMap::new(),
            registry: Registry::new(),
        }
    }

    /// Insert or replace by key. Replacing keeps the existing handle.
    pub fn set(&mut self, key: impl Into<String>, value: T) -> Handle {
        let key = key.into().to_ascii_lowercase();
        match self.handles.get(&key).copied() {
            Some(handle) => {
                if let Some(slot) = self.registry.get_mut(handle) {
                    *slot = value;
                    return handle;
                }
                let handle = self.registry.insert(value);
                self.handles.insert(key, handle);
                handle
            }
            None => {
                let handle = self.registry.insert(value);
                self.handles.insert(key, handle);
                handle
            }
        }
    }

    pub fn get(&self, key: &str) -> Option<&T> {
        let handle = self.handles.get(&key.to_ascii_lowercase())?;
        self.registry.get(*handle)
    }

    pub fn handle(&self, key: &str) -> Option<Handle> {
        self.handles.get(&key.to_ascii_lowercase()).copied()
    }

    pub fn iter(&self) -> impl Iterator<Item = (Handle, &T)> {
        self.registry.iter()
    }

    pub fn len(&self) -> usize {
        self.registry.len()
    }

    pub fn is_empty(&self) -> bool {
        self.registry.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_remove_roundtrip() {
        let mut registry = Registry::new();
        let first = registry.insert("alpha");
        let second = registry.insert("beta");
        assert_eq!(registry.get(first), Some(&"alpha"));
        assert_eq!(registry.get(second), Some(&"beta"));
        assert_eq!(registry.len(), 2);

        assert_eq!(registry.remove(first), Some("alpha"));
        assert_eq!(registry.get(first), None, "removed handle must not resolve");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn reused_slot_invalidates_old_handle() {
        let mut registry = Registry::new();
        let first = registry.insert(1);
        registry.remove(first);
        let reused = registry.insert(2);

        assert_eq!(registry.get(first), None);
        assert_eq!(registry.get(reused), Some(&2));
        assert_ne!(first, reused);
    }

    #[test]
    fn name_index_replaces_in_place() {
        let mut index = NameIndex::new();
        let handle = index.set("Demo Wash", 1);
        let replaced = index.set("demo wash", 2);

        assert_eq!(
            handle, replaced,
            "same case-insensitive key keeps the handle"
        );
        assert_eq!(index.get("DEMO WASH"), Some(&2));
        assert_eq!(index.len(), 1);
    }
}
