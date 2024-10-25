//! An "append store" is a storage wrapper that guarantees constant-cost appending to and popping
//! from a list of items in storage.
//!
//! This is achieved by storing each item in a separate storage entry. A special key is reserved
//! for storing the length of the collection so far.
use std::convert::TryInto;
use std::marker::PhantomData;

use serde::{de::DeserializeOwned, Serialize};

use cosmwasm_std::{StdError, StdResult, Storage};

use secret_toolkit::serialization::{Bincode2, Serde};

const LEN_KEY: &[u8] = b"len";

// Readonly append-store

/// A type allowing only reads from an append store. useful in the context_, u8 of queries.
#[derive(Debug)]
pub struct AppendStore<'a, T, S, Ser = Bincode2>
where
    T: Serialize + DeserializeOwned,
    S: Storage,
    Ser: Serde,
{
    storage: &'a S,
    item_type: PhantomData<*const T>,
    serialization_type: PhantomData<*const Ser>,
    len: u32,
}

impl<'a, T, S> AppendStore<'a, T, S, Bincode2>
where
    T: Serialize + DeserializeOwned,
    S: Storage,
{
    /// Try to use the provided storage as an AppendStore.
    ///
    /// Returns None if the provided storage doesn't seem like an AppendStore.
    /// Returns Err if the contents of the storage can not be parsed.
    pub fn attach(storage: &'a S) -> Option<StdResult<Self>> {
        AppendStore::attach_with_serialization(storage, Bincode2)
    }
}

impl<'a, T, S, Ser> AppendStore<'a, T, S, Ser>
where
    T: Serialize + DeserializeOwned,
    S: Storage,
    Ser: Serde,
{
    /// Try to use the provided storage as an AppendStore.
    /// This method allows choosing the serialization format you want to use.
    ///
    /// Returns None if the provided storage doesn't seem like an AppendStore.
    /// Returns Err if the contents of the storage can not be parsed.
    pub fn attach_with_serialization(storage: &'a S, _ser: Ser) -> Option<StdResult<Self>> {
        let len_vec = storage.get(LEN_KEY)?;
        Some(AppendStore::new(storage, len_vec))
    }

    fn new(storage: &'a S, len_vec: Vec<u8>) -> StdResult<Self> {
        let len_array = len_vec
            .as_slice()
            .try_into()
            .map_err(|err| StdError::parse_err("u32", err))?;
        let len = u32::from_be_bytes(len_array);

        Ok(Self {
            storage,
            item_type: PhantomData,
            serialization_type: PhantomData,
            len,
        })
    }

    pub fn len(&self) -> u32 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn readonly_storage(&self) -> &S {
        self.storage
    }

    /// Return an iterator over the items in the collection
    pub fn iter(&self) -> Iter<'a, T, S, Ser> {
        Iter {
            storage: AppendStore::clone(self),
            start: 0,
            end: self.len,
        }
    }

    /// Get the value stored at a given position.
    ///
    /// # Errors
    /// Will return an error if pos is out of bounds or if an item is not found.
    pub fn get_at(&self, pos: u32) -> StdResult<T> {
        if pos >= self.len {
            return Err(StdError::generic_err("AppendStorage access out of bounds"));
        }
        self.get_at_unchecked(pos)
    }

    fn get_at_unchecked(&self, pos: u32) -> StdResult<T> {
        let serialized = self.storage.get(&pos.to_be_bytes()).ok_or_else(|| {
            StdError::generic_err(format!("No item in AppendStorage at position {}", pos))
        })?;
        Ser::deserialize(&serialized)
    }
}

impl<'a, T, S, Ser> IntoIterator for AppendStore<'a, T, S, Ser>
where
    T: Serialize + DeserializeOwned,
    S: Storage,
    Ser: Serde,
{
    type Item = StdResult<T>;
    type IntoIter = Iter<'a, T, S, Ser>;

    fn into_iter(self) -> Iter<'a, T, S, Ser> {
        let end = self.len;
        Iter {
            storage: self,
            start: 0,
            end,
        }
    }
}

// Manual `Clone` implementation because the default one tries to clone the Storage??
impl<'a, T, S, Ser> Clone for AppendStore<'a, T, S, Ser>
where
    T: Serialize + DeserializeOwned,
    S: Storage,
    Ser: Serde,
{
    fn clone(&self) -> Self {
        Self {
            storage: &self.storage,
            item_type: self.item_type,
            serialization_type: self.serialization_type,
            len: self.len,
        }
    }
}

// Owning iterator

/// An iterator over the contents of the append store.
#[derive(Debug)]
pub struct Iter<'a, T, S, Ser>
where
    T: Serialize + DeserializeOwned,
    S: Storage,
    Ser: Serde,
{
    storage: AppendStore<'a, T, S, Ser>,
    start: u32,
    end: u32,
}

impl<'a, T, S, Ser> Iterator for Iter<'a, T, S, Ser>
where
    T: Serialize + DeserializeOwned,
    S: Storage,
    Ser: Serde,
{
    type Item = StdResult<T>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.start >= self.end {
            return None;
        }
        let item = self.storage.get_at(self.start);
        self.start += 1;
        Some(item)
    }

    // This needs to be implemented correctly for `ExactSizeIterator` to work.
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = (self.end - self.start) as usize;
        (len, Some(len))
    }

    // I implement `nth` manually because it is used in the standard library whenever
    // it wants to skip over elements, but the default implementation repeatedly calls next.
    // because that is very expensive in this case, and the items are just discarded, we wan
    // do better here.
    // In practice, this enables cheap paging over the storage by calling:
    // `append_store.iter().skip(start).take(length).collect()`
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        self.start = self.start.saturating_add(n as u32);
        self.next()
    }
}

impl<'a, T, S, Ser> DoubleEndedIterator for Iter<'a, T, S, Ser>
where
    T: Serialize + DeserializeOwned,
    S: Storage,
    Ser: Serde,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.start >= self.end {
            return None;
        }
        self.end -= 1;
        let item = self.storage.get_at(self.end);
        Some(item)
    }

    // I implement `nth_back` manually because it is used in the standard library whenever
    // it wants to skip over elements, but the default implementation repeatedly calls next_back.
    // because that is very expensive in this case, and the items are just discarded, we wan
    // do better here.
    // In practice, this enables cheap paging over the storage by calling:
    // `append_store.iter().skip(start).take(length).collect()`
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        self.end = self.end.saturating_sub(n as u32);
        self.next_back()
    }
}

// This enables writing `append_store.iter().skip(n).rev()`
impl<'a, T, S, Ser> ExactSizeIterator for Iter<'a, T, S, Ser>
where
    T: Serialize + DeserializeOwned,
    S: Storage,
    Ser: Serde,
{
}
