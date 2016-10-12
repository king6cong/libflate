/// Length-limited Huffman Codes
///
/// Reference: https://www.ics.uci.edu/~dan/pubs/LenLimHuff.pdf
use std::io;

use bit;
use bit::BitReader;

pub struct DecoderBuilder {
    table: Vec<u16>,
    eob_bitwidth: u8,
    max_bitwidth: u8,
}
impl DecoderBuilder {
    pub fn new(max_bitwidth: u8) -> Self {
        debug_assert!(max_bitwidth <= 15);
        DecoderBuilder {
            table: vec![0; 1 << max_bitwidth],
            eob_bitwidth: max_bitwidth,
            max_bitwidth: max_bitwidth,
        }
    }
    pub fn set_mapping(&mut self, bitwidth: u8, from: u16, to: u16) {
        debug_assert!(bitwidth > 0);
        debug_assert!(bitwidth <= self.max_bitwidth);
        if to == 256 {
            self.eob_bitwidth = bitwidth;
        }

        // Converts from little-endian to big-endian
        let mut from_le = from;
        let mut from_be = 0;
        for _ in 0..bitwidth {
            from_be <<= 1;
            from_be |= from_le & 1;
            from_le >>= 1;
        }

        // `bitwidth` encoded `to` value
        let value = (to << 4) | bitwidth as u16;

        // Sets the mapping to all possible indices
        for padding in 0..(1 << (self.max_bitwidth - bitwidth)) {
            let i = ((padding << bitwidth) | from_be) as usize;
            debug_assert_eq!(self.table[i], 0);
            unsafe {
                *self.table.get_unchecked_mut(i) = value;
            }
        }
    }
    pub fn finish(self) -> Decoder {
        Decoder {
            table: self.table,
            eob_bitwidth: self.eob_bitwidth,
            max_bitwidth: self.max_bitwidth,
        }
    }
    pub fn from_bitwidthes(bitwidthes: &[u8]) -> Decoder {
        debug_assert!(bitwidthes.len() > 0);

        // NOTE: Canonical Huffman Code
        let mut codes = Vec::new();
        for (code, count) in bitwidthes.iter().cloned().enumerate() {
            if count == 0 {
                continue;
            }
            codes.push((code as u16, count));
        }
        codes.sort_by_key(|x| x.1);

        let mut builder = Self::new(codes.last().unwrap().1);
        let mut from = 0;
        let mut prev_count = 0;
        for (code, count) in codes {
            if prev_count != count {
                from <<= count - prev_count;
                prev_count = count;
            }
            builder.set_mapping(count, from, code);
            from += 1;
        }
        builder.finish()
    }
}

pub struct Decoder {
    table: Vec<u16>,
    eob_bitwidth: u8,
    max_bitwidth: u8,
}
impl Decoder {
    #[inline]
    pub fn decode<R>(&mut self, reader: &mut BitReader<R>) -> io::Result<u16>
        where R: io::Read
    {
        // TODO: optimize
        let code = try!(reader.peek_bits(self.eob_bitwidth));
        let mut value = unsafe { *self.table.get_unchecked(code as usize) };
        let mut bitwidth = (value & 0b1111) as u8;

        // NOTE: bitwidth用のフィールドを5bitにすれば、最初の条件は無くせる
        if bitwidth == 0 || bitwidth > self.eob_bitwidth {
            let code = try!(reader.peek_bits(self.max_bitwidth));
            value = unsafe { *self.table.get_unchecked(code as usize) };
            bitwidth = (value & 0b1111) as u8;
            if bitwidth == 0 {
                return Err(invalid_data_error!("Invalid huffman coded stream"));
            }
        }
        let decoded = value >> 4;
        reader.skip_bits(bitwidth as u8);
        Ok(decoded)
    }
}

#[derive(Clone)]
struct Obj {
    codes: Vec<u16>,
    cost: usize,
}

#[derive(Debug)]
pub struct EncoderBuilder {
    table: Vec<(u8, u16)>,
}
impl EncoderBuilder {
    pub fn new(size: usize) -> Self {
        EncoderBuilder { table: vec![(0,0); size] }
    }
    pub fn set_mapping(&mut self, bitwidth: u8, from: u16, to: u16) {
        debug_assert_eq!(self.table[from as usize], (0, 0));

        // TODO: 共通化
        let mut to_le = to;
        let mut to_be = 0;
        for _ in 0..bitwidth {
            to_be <<= 1;
            to_be |= to_le & 1;
            to_le >>= 1;
        }

        self.table[from as usize] = (bitwidth, to_be);
    }
    pub fn finish(self) -> Encoder {
        Encoder { table: self.table }
    }
    pub fn from_frequencies(counts: &[usize], max_bitwidth: u8) -> Encoder {
        // (defun package-and-merge (objs next-objs)
        //   (merge 'list (packaging objs) next-objs #'< :key #'obj-cost))

        // (defmacro objs-each ((obj objs) &body body)
        //   (let ((self (gensym)))
        //     `(labels ((,self (,obj)
        //                 (if (packaged-obj-p ,obj)
        //                     (progn (,self (car (packaged-obj-pair ,obj)))
        //                            (,self (cdr (packaged-obj-pair ,obj))))
        //                   (progn ,@body))))
        //        (dolist (,obj ,objs)
        //          (,self ,obj)))))

        // (defun calc-code->b(#1=code-frequency-table bit-length-limit)
        //   (let ((src-objs (sort (loop FOR i FROM 0 BELOW (length #1#)
        //                               WHEN (plusp (aref #1# i))
        //                               COLLECT (make-code-obj :code i :cost (aref #1# i)))
        //                         #'< :key #'obj-cost))
        //         (bitlen-table (make-array (length #1#) :initial-element 0 :element-type 'octet)))
        //     (loop REPEAT bit-length-limit
        //           FOR objs = (package-and-merge objs (copy-list src-objs))
        //           FINALLY
        //             (objs-each (o (packaging objs))
        //               (incf (aref bitlen-table (code-obj-code o)))))
        //     bitlen-table))
        let mut src_objs = counts.iter()
            .cloned()
            .enumerate()
            .filter(|x| x.1 > 0)
            .map(|x| {
                Obj {
                    codes: vec![x.0 as u16],
                    cost: x.1,
                }
            })
            .collect::<Vec<_>>();
        src_objs.sort_by_key(|o| o.cost);
        let mut bitlen_table = vec![0; counts.len()];
        let mut objs = Vec::new();
        for _ in 0..max_bitwidth {
            objs = Self::package_and_merge(objs, src_objs.clone());
        }
        for code in Self::packaging(objs).into_iter().flat_map(|o| o.codes.into_iter()) {
            bitlen_table[code as usize] += 1;
        }
        Self::from_bitwidthes(&bitlen_table)
    }
    fn package_and_merge(objs: Vec<Obj>, src_objs: Vec<Obj>) -> Vec<Obj> {
        objs
    }
    fn packaging(objs: Vec<Obj>) -> Vec<Obj> {
        objs
    }
    pub fn from_bitwidthes(bitwidthes: &[u8]) -> Encoder {
        debug_assert!(bitwidthes.len() > 0);

        // NOTE: Canonical Huffman Code
        let mut codes = Vec::new();
        for (code, count) in bitwidthes.iter().cloned().enumerate() {
            if count == 0 {
                continue;
            }
            codes.push((code as u16, count));
        }
        codes.sort_by_key(|x| x.1);

        let mut builder = Self::new(codes.len());
        let mut to = 0;
        let mut prev_count = 0;
        for (code, count) in codes {
            if prev_count != count {
                to <<= count - prev_count;
                prev_count = count;
            }
            builder.set_mapping(count, code, to);
            to += 1;
        }
        builder.finish()
    }
}


#[derive(Debug, Clone)]
pub struct Encoder {
    // XXX:
    pub table: Vec<(u8, u16)>,
}
impl Encoder {
    pub fn encode<W>(&mut self, writer: &mut bit::BitWriter<W>, code: u16) -> io::Result<()>
        where W: io::Write
    {
        debug_assert!(self.table[code as usize] != (0, 0));
        let (bitwidth, encoded) = self.table[code as usize];
        writer.write_bits(bitwidth, encoded)
    }
    pub fn used_max_code(&self) -> Option<u16> {
        self.table
            .iter()
            .rev()
            .position(|x| x.0 > 0)
            .map(|trailing_zeros| (self.table.len() - 1 - trailing_zeros) as u16)
    }
}

#[cfg(test)]
mod test {
    #[test]
    fn it_works() {}
}
