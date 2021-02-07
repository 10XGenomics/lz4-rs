use crate::liblz4::*;
use crate::size_t;
use std::io::{Error, ErrorKind, Result};
use std::ptr;
use std::task::Poll;
use tokio::io::AsyncRead;
use tokio::io::ReadBuf;

const BUFFER_SIZE: usize = 32 * 1024;

struct DecoderContext {
    c: LZ4FDecompressionContext,
}

pub struct AsyncDecoder<R> {
    c: DecoderContext,
    /// underying AsyncReader, e.g. file
    r: R,
    /// undecoded bytes filled from r
    ///  <- decoded ->  <- not decoded yet -->
    /// [0......(pos-1)|(pos)..........(len-1)|(len)...]
    buf: Box<[u8]>, 
    /// next position of buf[] to decode
    pos: usize,
    len: usize,
    /// minimum next bytes remaining. we don't read more than this.
    next: usize,  
}

impl<R: AsyncRead> AsyncDecoder<R> {
    /// Creates a new encoder which will have its output written to the given
    /// output stream. The output stream can be re-acquired by calling
    /// `finish()`
    pub fn new(r: R) -> Result<AsyncDecoder<R>> {
        Ok(AsyncDecoder {
            r,
            c: DecoderContext::new()?,
            buf: vec![0; BUFFER_SIZE].into_boxed_slice(),
            pos: BUFFER_SIZE,
            len: BUFFER_SIZE,
            // 11 is Minimal LZ4 stream size
            next: 11,
        })
    }

    /// Immutable reader reference.
    pub fn reader(&self) -> &R {
        &self.r
    }

    pub fn finish(self) -> (R, Result<()>) {
        (
            self.r,
            match self.next {
                0 => Ok(()),
                _ => Err(Error::new(
                    ErrorKind::Interrupted,
                    "Finish runned before read end of compressed stream",
                )),
            },
        )
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for AsyncDecoder<R> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context,
        buf: &mut ReadBuf, // fill buf with decoded data
    ) -> Poll<Result<()>> {
        if self.next == 0 || buf.remaining() == 0 {  // EOF or dst buffer is full
            return Poll::Ready(Ok(()));
        }
        let s = self.get_mut();
        while buf.filled().len() == 0 {  // no decoded bytes are produced yet
            if s.pos >= s.len {  // raw buffer is empty => read from underying buffer
                let need = if s.buf.len() < s.next {
                    s.buf.len()
                } else {
                    s.next
                };

                let underying_reader = std::pin::Pin::new(&mut s.r);
                let mut r = ReadBuf::new(&mut s.buf[0..need]);
                match underying_reader.poll_read(cx, &mut r) {
                    Poll::Ready(Ok(())) => {
                        if r.remaining() == r.capacity() {
                            // underying reader is at EOF
                            s.next = 0;
                            return Poll::Ready(Ok(()));
                        }
                    }
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                }
                s.pos = 0;
                s.len = r.filled().len();
                s.next -= s.len;
            }
            while buf.remaining()>0 && (s.pos < s.len) {
                let mut src_size = (s.len - s.pos) as size_t;
                let mut dst_size = buf.remaining() as size_t;
                let dst_mem = buf.initialize_unfilled();
                let len = check_error(unsafe {
                    LZ4F_decompress(
                        s.c.c,
                        dst_mem.as_mut_ptr(),
                        &mut dst_size,
                        s.buf[s.pos..].as_ptr(),
                        &mut src_size,
                        ptr::null(),
                    )
                })?;
                s.pos += src_size as usize;
                buf.advance(dst_size);
                if len == 0 {  // raw bytes are not consumed
                    s.next = 0;
                    return Poll::Ready(Ok(()));
                } else if s.next < len {
                    s.next = len;
                }
            }
        }
        Poll::Ready(Ok(()))
    }
}

impl DecoderContext {
    fn new() -> Result<DecoderContext> {
        let mut context = LZ4FDecompressionContext(ptr::null_mut());
        check_error(unsafe { LZ4F_createDecompressionContext(&mut context, LZ4F_VERSION) })?;
        Ok(DecoderContext { c: context })
    }
}

impl Drop for DecoderContext {
    fn drop(&mut self) {
        unsafe { LZ4F_freeDecompressionContext(self.c) };
    }
}
