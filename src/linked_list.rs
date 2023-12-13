use std::marker::PhantomData;

use near_sdk::{
    borsh::{self, BorshDeserialize, BorshSerialize},
    env, BorshStorageKey, IntoStorageKey,
};

#[derive(BorshSerialize, BorshStorageKey)]
enum StorageKey {
    Item(u64),
}

#[derive(BorshSerialize, BorshDeserialize)]
struct Item<T> {
    value: T,
    next: Option<u64>,
}

/// A data structure for queue-like operations.
#[derive(BorshSerialize, BorshDeserialize)]
pub struct LinkedList<T: BorshSerialize> {
    _marker: PhantomData<T>,
    prefix: Box<[u8]>,
    len: u32,
    ends: Option<(u64, u64)>,
    next_id: u64,
}

impl<T: BorshSerialize> LinkedList<T> {
    pub fn new<S: IntoStorageKey>(prefix: S) -> Self {
        Self {
            _marker: PhantomData,
            prefix: prefix.into_storage_key().into(),
            len: 0,
            ends: None,
            next_id: 0,
        }
    }

    fn new_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .unwrap_or_else(|| env::panic_str("Overflow"));
        id
    }

    pub fn len(&self) -> u32 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[inline]
    fn id_to_key(&self, id: u64) -> Vec<u8> {
        StorageKey::Item(id).try_to_vec().unwrap()
    }
}

impl<T: BorshSerialize + BorshDeserialize> LinkedList<T> {
    fn get_item(&self, id: u64) -> Item<T> {
        let key = self.id_to_key(id);
        BorshDeserialize::try_from_slice(&env::storage_read(&key).unwrap()).unwrap()
    }

    fn set_item(&mut self, id: u64, item: Item<T>) {
        let key = self.id_to_key(id);
        env::storage_write(&key, &BorshSerialize::try_to_vec(&item).unwrap());
    }

    fn take_item(&mut self, id: u64) -> Item<T> {
        let key = self.id_to_key(id);
        env::storage_remove(&key);
        let value = env::storage_get_evicted().unwrap();
        BorshDeserialize::try_from_slice(&value).unwrap()
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            list: self,
            next_id: self.ends.map(|(head_id, _)| head_id),
        }
    }

    pub fn peek(&self) -> Option<T> {
        self.ends.map(|(head_id, _)| self.get_item(head_id).value)
    }

    /// Replace the head of the list. If the list is empty, do nothing.
    pub fn replace(&mut self, value: T) {
        if let Some((head_id, _)) = self.ends {
            let item = self.get_item(head_id);
            self.set_item(head_id, Item { value, ..item });
        }
    }

    pub fn peek_back(&self) -> Option<T> {
        self.ends.map(|(_, tail_id)| self.get_item(tail_id).value)
    }

    pub fn prepend(&mut self, value: T) {
        let id = self.new_id();
        self.len += 1;

        let mut item = Item { value, next: None };

        match self.ends {
            None => {
                self.ends = Some((id, id));
            }
            Some((head_id, tail_id)) => {
                item.next = Some(head_id);

                self.ends = Some((id, tail_id));
            }
        }

        self.set_item(id, item);
    }

    pub fn dequeue(&mut self) -> Option<T> {
        match self.ends {
            None => None,
            Some((head_id, tail_id)) => {
                self.len -= 1;

                let head = self.take_item(head_id);

                if head_id == tail_id {
                    self.ends = None;
                    self.next_id = 0;
                } else {
                    self.ends = Some((head.next.unwrap(), tail_id));
                }

                Some(head.value)
            }
        }
    }

    pub fn enqueue(&mut self, value: T) {
        let id = self.new_id();
        self.len += 1;

        match self.ends {
            None => {
                self.ends = Some((id, id));
            }
            Some((head_id, tail_id)) => {
                let mut tail: Item<T> = self.get_item(tail_id);
                tail.next = Some(id);
                self.set_item(tail_id, tail);

                self.ends = Some((head_id, id));
            }
        }

        let item = Item { value, next: None };

        self.set_item(id, item);
    }
}

pub struct Iter<'a, T: BorshSerialize + BorshDeserialize> {
    list: &'a LinkedList<T>,
    next_id: Option<u64>,
}

impl<'a, T: BorshSerialize + BorshDeserialize> Iterator for Iter<'a, T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_id.map(|id| {
            let item = self.list.get_item(id);
            self.next_id = item.next;
            item.value
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_trivial() {
        let mut q = LinkedList::<u32>::new(b"q");
        assert_eq!(q.dequeue(), None);
        assert_eq!(q.len(), 0);
        assert!(q.is_empty());
        q.enqueue(1);
        assert_eq!(q.len(), 1);
        assert!(!q.is_empty());
        q.enqueue(2);
        assert_eq!(q.len(), 2);
        q.enqueue(3);
        assert_eq!(q.len(), 3);
        assert_eq!(q.dequeue(), Some(1));
        assert_eq!(q.len(), 2);
        assert_eq!(q.dequeue(), Some(2));
        assert_eq!(q.len(), 1);
        assert_eq!(q.dequeue(), Some(3));
        assert_eq!(q.len(), 0);
        assert_eq!(q.dequeue(), None);
        assert_eq!(q.len(), 0);
        assert!(q.is_empty());
    }

    #[test]
    fn test_queue_interleaved() {
        let mut q = LinkedList::<u32>::new(b"q");
        q.enqueue(1);
        assert_eq!(q.dequeue(), Some(1));
        assert_eq!(q.dequeue(), None);
        q.enqueue(2);
        q.enqueue(3);
        assert_eq!(q.dequeue(), Some(2));
        q.enqueue(4);
        q.enqueue(5);
        q.enqueue(6);
        q.enqueue(7);
        assert_eq!(q.dequeue(), Some(3));
        assert_eq!(q.len(), 4);
        assert_eq!(q.dequeue(), Some(4));
        assert_eq!(q.len(), 3);
        assert_eq!(q.dequeue(), Some(5));
        assert_eq!(q.len(), 2);
        assert_eq!(q.dequeue(), Some(6));
        assert_eq!(q.len(), 1);
        assert_eq!(q.dequeue(), Some(7));
        assert_eq!(q.len(), 0);
        assert_eq!(q.dequeue(), None);
        assert_eq!(q.len(), 0);
        q.enqueue(4);
        q.enqueue(5);
        q.enqueue(6);
        q.enqueue(7);
        q.enqueue(8);
        assert_eq!(q.dequeue(), Some(4));
        assert_eq!(q.dequeue(), Some(5));
        assert_eq!(q.dequeue(), Some(6));
        assert_eq!(q.dequeue(), Some(7));
        assert_eq!(q.dequeue(), Some(8));
        assert_eq!(q.dequeue(), None);
        assert!(q.is_empty());
    }

    #[test]
    fn lots_of_items() {
        let mut q = LinkedList::<u32>::new(b"q");
        for i in 0..100 {
            q.enqueue(i);
        }
        for i in 0..100 {
            assert_eq!(q.dequeue(), Some(i));
        }
        assert_eq!(q.dequeue(), None);
    }
}
