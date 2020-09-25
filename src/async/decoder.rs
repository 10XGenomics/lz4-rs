use crate::liblz4::*;
use crate::size_t;
use std::io::{Error, ErrorKind, Result};
use std::ptr;
use std::task::Poll;
use tokio::io::AsyncRead;

const BUFFER_SIZE: usize = 32 * 1024;

struct DecoderContext {
    c: LZ4FDecompressionContext,
}

pub struct AsyncDecoder<R> {
    c: DecoderContext,
    r: R,
    buf: Box<[u8]>,
    pos: usize,
    len: usize,
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
            // Minimal LZ4 stream size
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
        buf: &mut [u8],
    ) -> Poll<Result<usize>> {
        if self.next == 0 || buf.is_empty() {
            return Poll::Ready(Ok(0));
        }
        let s = self.get_mut();
        let mut dst_offset: usize = 0;
        while dst_offset == 0 {
            if s.pos >= s.len {
                let need = if s.buf.len() < s.next {
                    s.buf.len()
                } else {
                    s.next
                };

                let r = std::pin::Pin::new(&mut s.r);
                match r.poll_read(cx, &mut s.buf[0..need]) {
                    Poll::Ready(Ok(0)) => break,
                    Poll::Ready(Ok(n)) => s.len = n,
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                }
                s.pos = 0;
                s.next -= s.len;
            }
            while (dst_offset < buf.len()) && (s.pos < s.len) {
                let mut src_size = (s.len - s.pos) as size_t;
                let mut dst_size = (buf.len() - dst_offset) as size_t;
                let len = check_error(unsafe {
                    LZ4F_decompress(
                        s.c.c,
                        buf[dst_offset..].as_mut_ptr(),
                        &mut dst_size,
                        s.buf[s.pos..].as_ptr(),
                        &mut src_size,
                        ptr::null(),
                    )
                })?;
                s.pos += src_size as usize;
                dst_offset += dst_size as usize;
                if len == 0 {
                    s.next = 0;
                    return Poll::Ready(Ok(dst_offset));
                } else if s.next < len {
                    s.next = len;
                }
            }
        }
        Poll::Ready(Ok(dst_offset))
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
