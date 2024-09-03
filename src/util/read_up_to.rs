use std::io::{BufRead, ErrorKind};

use tracing::trace;

pub trait ReadUpTo: BufRead {
    fn read_up_to(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}

impl<B: BufRead> ReadUpTo for B {
    /// Continuously reads data until the given buffer is filled or the read times out/would block,
    /// in which case the amount of bytes read is returned. It is possible the returned amount
    /// is 0 if the first read already times out/would block. If an `ErrorKind::Interrupted` is
    /// encountered, it is ignored. Any other errors are returned.
    fn read_up_to(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut total = 0;
        loop {
            if total == buf.len() {
                return Ok(total);
            }

            let read = match self.fill_buf() {
                Ok(read) if !read.is_empty() => read,
                // i guess it's possible for the read to return nothing so return early if so
                Ok(_) => return Ok(total),

                Err(e) if timed_out_or_would_block(e.kind()) => return Ok(total),
                Err(e) if e.kind() == ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            };

            let read_len = read.len(); // how much was read
            let remaining = buf.len() - total; // how much space is left in the buffer
            let copy = read_len.min(remaining); // how much should be copied (and consumed)

            trace!(
                read_len,
                total,
                remaining,
                copy,
                ?read,
                "read serial device"
            );

            buf[total..total + copy].copy_from_slice(&read[..copy]);
            self.consume(copy);
            total += copy;
        }
    }
}

fn timed_out_or_would_block(kind: ErrorKind) -> bool {
    kind == ErrorKind::TimedOut || kind == ErrorKind::WouldBlock
}
