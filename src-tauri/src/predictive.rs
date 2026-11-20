//! TufaZIP Local v1 block codec. All model updates and interval operations use integers.
//! Each block starts with a fresh model so decoding is independent of hardware and scheduling.
const HALF: u64 = 1 << 31;
const QUARTER: u64 = 1 << 30;
const TOP: u64 = (1 << 32) - 1;

struct Model {
    base: [u16; 256],
    order1: Vec<u16>,
    order2: Vec<u16>,
    history: usize,
}

impl Model {
    fn new(level: u8) -> Self {
        Self {
            base: [2048; 256],
            order1: vec![2048; if level >= 2 { 65536 } else { 0 }],
            order2: vec![2048; if level >= 3 { 262144 } else { 0 }],
            history: 0,
        }
    }

    fn indices(&self, node: usize) -> (usize, usize) {
        (
            (self.history & 255) * 256 + node,
            ((self.history.wrapping_mul(40503) >> 6) & 1023) * 256 + node,
        )
    }

    fn probability(&self, node: usize) -> u64 {
        let (first, second) = self.indices(node);
        let mut p = u64::from(self.base[node]);
        if !self.order1.is_empty() {
            p = (p + 3 * u64::from(self.order1[first])) / 4;
        }
        if !self.order2.is_empty() {
            p = (p + u64::from(self.order2[second])) / 2;
        }
        p
    }

    fn update(&mut self, node: usize, bit: u8) {
        fn adapt(p: &mut u16, bit: u8) {
            if bit == 0 {
                *p += (4096 - *p) >> 5;
            } else {
                *p -= *p >> 5;
            }
        }
        let (first, second) = self.indices(node);
        adapt(&mut self.base[node], bit);
        if !self.order1.is_empty() {
            adapt(&mut self.order1[first], bit);
        }
        if !self.order2.is_empty() {
            adapt(&mut self.order2[second], bit);
        }
    }

    fn finish_byte(&mut self, byte: u8) {
        self.history = ((self.history << 8) | usize::from(byte)) & 65535;
    }
}

struct Interval {
    low: u64,
    high: u64,
}
impl Interval {
    fn new() -> Self {
        Self { low: 0, high: TOP }
    }
    fn split(&self, p: u64) -> u64 {
        self.low + (((self.high - self.low + 1) * p) >> 12)
    }
    fn narrow(&mut self, split: u64, bit: u8) {
        if bit == 0 {
            self.high = split - 1;
        } else {
            self.low = split;
        }
    }
    fn shift(&mut self, offset: u64) {
        self.low = (self.low - offset) * 2;
        self.high = (self.high - offset) * 2 + 1;
    }
    fn offset(&self) -> Option<u64> {
        if self.high < HALF {
            Some(0)
        } else if self.low >= HALF {
            Some(HALF)
        } else if self.low >= QUARTER && self.high < 3 * QUARTER {
            Some(QUARTER)
        } else {
            None
        }
    }
}

#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    count: usize,
    pending: usize,
}
impl Bits {
    fn push(&mut self, bit: u8) {
        let shift = 7 - self.count % 8;
        if shift == 7 {
            self.bytes.push(0);
        }
        *self.bytes.last_mut().unwrap() |= bit << shift;
        self.count += 1;
    }
    fn emit(&mut self, bit: u8) {
        self.push(bit);
        for _ in 0..self.pending {
            self.push(1 - bit);
        }
        self.pending = 0;
    }
}

pub fn encode(input: &[u8], level: u8) -> Vec<u8> {
    let mut model = Model::new(level);
    let mut interval = Interval::new();
    let mut bits = Bits::default();
    for &byte in input {
        let mut node = 1;
        for shift in (0..8).rev() {
            let bit = (byte >> shift) & 1;
            interval.narrow(interval.split(model.probability(node)), bit);
            model.update(node, bit);
            node = node * 2 + usize::from(bit);
            while let Some(offset) = interval.offset() {
                if offset == QUARTER {
                    bits.pending += 1;
                } else {
                    bits.emit(u8::from(offset == HALF));
                }
                interval.shift(offset);
            }
        }
        model.finish_byte(byte);
    }
    bits.pending += 1;
    bits.emit(u8::from(interval.low >= QUARTER));
    bits.bytes.extend_from_slice(&[0; 4]); // Decoder lookahead at the end of the interval.
    bits.bytes
}

pub fn decode(input: &[u8], size: usize, level: u8) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context;
    let mut pos = 0;
    let mut next = || -> anyhow::Result<u64> {
        let byte = input.get(pos / 8).context("damaged")?;
        let bit = (byte >> (7 - pos % 8)) & 1;
        pos += 1;
        Ok(u64::from(bit))
    };
    let mut code = 0;
    for _ in 0..32 {
        code = (code << 1) | next()?;
    }
    let mut interval = Interval::new();
    let mut model = Model::new(level);
    let mut output = Vec::with_capacity(size);
    for _ in 0..size {
        let mut node = 1;
        for _ in 0..8 {
            let split = interval.split(model.probability(node));
            let bit = u8::from(code >= split);
            interval.narrow(split, bit);
            model.update(node, bit);
            node = node * 2 + usize::from(bit);
            while let Some(offset) = interval.offset() {
                code = ((code - offset) << 1) | next()?;
                interval.shift(offset);
            }
        }
        let byte = (node - 256) as u8;
        output.push(byte);
        model.finish_byte(byte);
    }
    Ok(output)
}
