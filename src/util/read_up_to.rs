use std::io::{BufRead, ErrorKind};

pub trait ReadUpTo: BufRead {
    fn read_up_to(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}

impl<B: BufRead> ReadUpTo for B {
    /// Continuously reads data until the given buffer is filled or the read is timed out, in which
    /// case the amount of bytes read is returned. It is possible the returned amount is 0 if the
    /// first read times out. If an `ErrorKind::Interrupted` is encountered, it is ignored. Any
    /// other errors are returned.
    fn read_up_to(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut total = 0;
        loop {
            if total == buf.len() {
                return Ok(total);
            }

            let read = match self.fill_buf() {
                Ok(read) => read,
                Err(e) if e.kind() == ErrorKind::TimedOut => return Ok(total),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };

            let target = &mut buf[total..];
            let amt = read.len();
            let len = target.len();

            // println!("r:{amt} t:{total} l:{len}");

            if amt < len {
                target[..amt].copy_from_slice(read);
                self.consume(amt);
                total += amt;
            } else {
                target.copy_from_slice(&read[..len]);
                self.consume(len);
                total += len;
            }
        }
    }
}
