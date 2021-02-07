use crate::liblz4::*;
use crate::size_t;
use std::io::{Error, ErrorKind, Result};
use std::ptr;
use std::task::Poll;
use tokio::io::AsyncRead;
use tokio::io::ReadBuf;

const BUFFER_SIZE: usize = 32 * 1024;

struct DecoderContext {
    inner: LZ4FDecompressionContext,
}

pub struct AsyncDecoder<R> {
    decoder_context: DecoderContext,
    /// underying AsyncReader, e.g. file
    r: R,
    /// undecoded bytes filled from r
    ///  <- decoded ->  <- not decoded yet -->
    /// [0......(pos-1)|(pos)..........(len-1)|(len)...]
    raw_buf: Box<[u8]>, 
    pos: usize,
    len: usize,
    /// minimum next bytes remaining. we don't read more than this.
    next_raw_bytes_expected_by_decoder: usize,  
}

impl<R: AsyncRead> AsyncDecoder<R> {
    /// Creates a new encoder which will have its output written to the given
    /// output stream. The output stream can be re-acquired by calling
    /// `finish()`
    pub fn new(r: R) -> Result<AsyncDecoder<R>> {
        Ok(AsyncDecoder {
            r,
            decoder_context: DecoderContext::new()?,
            raw_buf: vec![0; BUFFER_SIZE].into_boxed_slice(),
            pos: BUFFER_SIZE,
            len: BUFFER_SIZE,
            // 11 is Minimal LZ4 stream size
            next_raw_bytes_expected_by_decoder: 11,
        })
    }

    /// Immutable reader reference.
    pub fn reader(&self) -> &R {
        &self.r
    }

    pub fn finish(self) -> (R, Result<()>) {
        (
            self.r,
            match self.next_raw_bytes_expected_by_decoder {
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
        dst_buf: &mut ReadBuf, // fill buf with decoded data
    ) -> Poll<Result<()>> {
        if self.next_raw_bytes_expected_by_decoder == 0 || dst_buf.remaining() == 0 {  // EOF or dst buffer is full
            return Poll::Ready(Ok(()));
        }
        let slf = self.get_mut();
        while dst_buf.remaining()>0 {
            if slf.pos == slf.len {
                // raw buffer is empty => read more from underying buffer
                let raw_bytes_to_read = BUFFER_SIZE.min(slf.next_raw_bytes_expected_by_decoder);
                let underying_reader = std::pin::Pin::new(&mut slf.r);
                let mut r = ReadBuf::new(&mut slf.raw_buf[0..raw_bytes_to_read]);
                match underying_reader.poll_read(cx, &mut r) {
                    Poll::Ready(Ok(())) => {
                        // Lz4 expects some bytes but inner reader is EOF
                        if r.filled().len() == 0 {
                            if slf.next_raw_bytes_expected_by_decoder > 0 {
                                return Poll::Ready(
                                    Err(Error::new(
                                        ErrorKind::UnexpectedEof,
                                        "Lz4 Decoder expects more bytes"))
                                );
                            } else {
                                // normal EOF
                                break;
                            }
                        }
                    }
                    Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                    Poll::Pending => return Poll::Pending,
                }

                slf.pos = 0;
                slf.len = r.filled().len();
            }
            let mut src_buf_remaining = (slf.len - slf.pos) as size_t; // >=0
            let mut dst_buf_remaining = dst_buf.remaining() as size_t; // > 0
            let dst_mem = dst_buf.initialize_unfilled();
            let next_expected_src_bytes = check_error(unsafe {
                LZ4F_decompress(
                    slf.decoder_context.inner,
                    dst_mem.as_mut_ptr(),
                    &mut dst_buf_remaining,
                    slf.raw_buf[slf.pos..slf.len].as_ptr(),
                    &mut src_buf_remaining,
                    ptr::null(),
                )
            })?;
            let input_consumed=  src_buf_remaining; drop(src_buf_remaining);
            let output_produced = dst_buf_remaining; drop(dst_buf_remaining);
            slf.pos += input_consumed;
            dst_buf.advance(output_produced);
            slf.next_raw_bytes_expected_by_decoder = next_expected_src_bytes;
            if output_produced > 0 { 
                break;
            }
            if input_consumed == 0 || next_expected_src_bytes == 0 {
                break;
            }
            
        }
        Poll::Ready(Ok(()))
    }
}

impl DecoderContext {
    fn new() -> Result<DecoderContext> {
        let mut context = LZ4FDecompressionContext(ptr::null_mut());
        check_error(unsafe { LZ4F_createDecompressionContext(&mut context, LZ4F_VERSION) })?;
        Ok(DecoderContext { inner: context })
    }
}

impl Drop for DecoderContext {
    fn drop(&mut self) {
        unsafe { LZ4F_freeDecompressionContext(self.inner) };
    }
}
