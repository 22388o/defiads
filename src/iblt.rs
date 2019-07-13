//! Iterable Bloom Lookup Table
//! see: https://dash.harvard.edu/bitstream/handle/1/14398536/GENTILI-SENIORTHESIS-2015.pdf

use std::collections::vec_deque::VecDeque;
use std::io::Write;
use std::error::Error;
use std::fmt;

use bitcoin_hashes::siphash24;
use rand::{RngCore, thread_rng};
use byteorder::{WriteBytesExt, BigEndian};

const ID_LEN:usize = 32;

#[derive(Clone)]
pub struct IBLT {
    buckets: Vec<Bucket>,
    k0: u64,
    k1: u64,
    k: u8
}

#[derive(Default,Clone)]
struct Bucket {
    keysum: [u8; ID_LEN],
    keyhash: u64,
    counter: i32
}

impl IBLT {

    /// Create a new IBLT with m buckets and k hash functions
    pub fn new (m: usize, k: u8) -> IBLT {
        let mut rnd = thread_rng();
        IBLT{buckets: vec![Bucket::default();m], k0: rnd.next_u64(), k1: rnd.next_u64(), k}
    }

    fn hash (k0: u64, k1: u64, id: &[u8]) -> u64 {
        let mut engine = siphash24::HashEngine::with_keys(k0, k1);
        engine.write_all(id).unwrap();
        siphash24::Hash::from_engine_to_u64(engine)
    }

    /// insert an id
    pub fn insert (&mut self, id: &[u8]) {
        assert_eq!(id.len(), ID_LEN);
        let keyhash = Self::hash(self.k1, self.k0, id);
        let mut hash = self.k0;
        for _ in 0..self.k {
            hash = Self::hash(hash, self.k1, id);
            let n = IBLT::fast_reduce(hash, self.buckets.len());
            let ref mut bucket = self.buckets[n];
            for i in 0..id.len () {
                bucket.keysum[i] ^= id[i];
            }
            bucket.counter += 1;
            bucket.keyhash ^= keyhash;
        }
    }

    /// delete an id
    pub fn delete (&mut self, id: &[u8]) {
        assert_eq!(id.len(), ID_LEN);
        let keyhash = Self::hash(self.k1, self.k0, id);
        let mut hash = self.k0;
        for _ in 0..self.k {
            hash = Self::hash(hash, self.k1, id);
            let n = IBLT::fast_reduce(hash, self.buckets.len());
            let ref mut bucket = self.buckets[n];
            for i in 0..id.len () {
                bucket.keysum[i] ^= id[i];
            }
            bucket.counter -= 1;
            bucket.keyhash ^= keyhash;
        }
    }

    /// iterate over ids. This preserves the IBLT as it makes a copy internally
    pub fn iter(&self, added: bool) -> IBLTIterator {
        IBLTIterator::new(self.clone(), added)
    }

    /// iterare over ids. This destroys the IBLT
    pub fn into_iter (self, added: bool) -> IBLTIterator {
        IBLTIterator::new(self, added)
    }

    /// return an itartor of ids missing in this one but is itarable from the other IBLT
    pub fn missing (&mut self, other: &mut IBLTIterator) -> Result<IBLTIterator, IBLTError> {
        let mut copy = self.clone();
        for id in other {
            copy.delete(&id?[..]);
        }
        Ok(copy.into_iter(false))
    }

    pub fn is_overloaded (&self) -> bool {
        self.iter(true).any(|e| e.is_err())
    }

    fn fast_reduce (n: u64, r: usize) -> usize {
        ((n as u128 * r as u128) >> 64) as usize
    }
}

#[derive(Debug)]
pub enum IBLTError {
    IncompleteIteration
}

impl Error for IBLTError {
    fn description(&self) -> &str {
        "Incomplete IBLT iteration"
    }
}

impl fmt::Display for IBLTError {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        write!(f, "{}", self.description())
    }
}

pub struct IBLTIterator {
    iblt: IBLT,
    queue: VecDeque<usize>,
    one: i32
}

impl IBLTIterator {
    pub fn new (iblt: IBLT, added: bool) -> IBLTIterator {
        let one = if added { 1 } else { -1 };
        let mut queue = VecDeque::new();
        for (i, bucket) in iblt.buckets.iter().enumerate() {
            if bucket.counter.abs() == 1 &&
                bucket.keyhash == IBLT::hash(iblt.k1, iblt.k0, &bucket.keysum[..]) {
                queue.push_back(i);
            }
        }
        IBLTIterator{iblt, queue, one}
    }
}

impl Iterator for IBLTIterator {
    type Item = Result<[u8; ID_LEN], IBLTError>;

    fn next(&mut self) -> Option<Self::Item> {
        while let Some(i) = self.queue.pop_front() {
            let c = self.iblt.buckets[i].counter;
            if c.abs() == 1 {
                let id = self.iblt.buckets[i].keysum;
                let keyhash = IBLT::hash(self.iblt.k1, self.iblt.k0, &id[..]);
                let found = c == self.one && keyhash == self.iblt.buckets[i].keyhash;
                let mut hash = self.iblt.k0;
                for _ in 0..self.iblt.k {
                    hash = IBLT::hash(hash, self.iblt.k1, &id[..]);
                    let n = IBLT::fast_reduce(hash, self.iblt.buckets.len());
                    let ref mut bucket = self.iblt.buckets[n];
                    for i in 0..id.len () {
                        bucket.keysum[i] ^= id[i];
                    }
                    bucket.counter -= c;
                    bucket.keyhash ^= keyhash;
                    if bucket.counter.abs() == 1 &&
                        IBLT::hash(self.iblt.k1, self.iblt.k0, &bucket.keysum[..]) == bucket.keyhash {
                        self.queue.push_back(n);
                    }
                }
                if found {
                    return Some(Ok(id));
                }
            }
        }
        for bucket in &self.iblt.buckets {
            if bucket.counter != 0  {
                return Some(Err(IBLTError::IncompleteIteration));
            }
        }
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::HashSet;

    #[test]
    pub fn test_single_insert () {
        let mut a = IBLT::new(10, 3);

        a.insert(&[1; ID_LEN]);
        assert_eq!(a.iter(true).next().unwrap().unwrap(), [1; ID_LEN]);
    }

    #[test]
    pub fn test_single_insert_delete () {
        let mut a = IBLT::new(10, 3);

        a.insert(&[1; ID_LEN]);
        a.delete(&[1; ID_LEN]);
        assert!(a.iter(true).next().is_none());
    }

    #[test]
    pub fn test_few_inserts () {
        let mut a = IBLT::new(1000, 3);

        let mut set = HashSet::new();
        for i in 0..20 {
            set.insert([i; ID_LEN]);
            a.insert(&[i; ID_LEN]);
        }

        for id in a.iter(true) {
            assert! (set.remove(&id.unwrap()));
        }
        assert!(set.is_empty());
    }

    #[test]
    pub fn test_few_inserts_deletes () {
        let mut a = IBLT::new(1000, 3);

        let mut inserted = HashSet::new();
        let mut removed = HashSet::new();
        for i in 0..20 {
            inserted.insert([i; ID_LEN]);
            a.insert(&[i; ID_LEN]);
        }
        for i in 10 .. 30 {
            removed.insert([i; ID_LEN]);
            a.delete(&[i; ID_LEN]);
        }

        let mut remained = inserted.difference(&removed).collect::<HashSet<_>>();

        for id in a.iter(true) {
            assert! (remained.remove(&id.unwrap()));
        }
        assert!(remained.is_empty());

        let mut deleted = removed.difference(&inserted).collect::<HashSet<_>>();
        for id in a.iter(false) {
            assert! (deleted.remove(&id.unwrap()));
        }
        assert!(deleted.is_empty());
    }



    #[test]
    pub fn test_missing () {
        let mut a = IBLT::new(1000, 3);
        let mut b = IBLT::new(1000, 3);

        for i in 0..20 {
            a.insert(&[i; ID_LEN]);
            if i < 15 {
                b.insert(&[i; ID_LEN])
            }
        }

        for m in  b.missing(&mut a.into_iter(true)).unwrap() {
            b.insert(&m.unwrap()[..]);
        }

        assert_eq!(b.iter(true).count(), 20)
    }

    #[test]
    pub fn test_overload() {
        let mut a = IBLT::new(10, 5);
        for i in 0..20 {
            a.insert(&[i; ID_LEN]);
        }
        assert!(a.is_overloaded());
    }
}