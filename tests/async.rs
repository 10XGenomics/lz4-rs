use std::io::{Cursor, Seek, SeekFrom};
use std::io::{Write};

use tokio::io::AsyncReadExt;

#[test]
fn test_async_decoder() {
    let mut rng = rand::thread_rng();
    fn random_stream<R: rand::Rng>(rng: &mut R, size: usize) -> Vec<u8> {
        (0..size).map(|_| rng.gen()).collect()
    }
    let data = random_stream(&mut rng, 1024);

    let mut buf = Cursor::new(vec![0;5*1024]);
    {
        let mut w = lz4::EncoderBuilder::new().build(&mut buf).unwrap();
        w.write_all(data.as_slice()).unwrap();
        // w.write_all("abc def ghi".as_bytes()).unwrap();
        w.finish().1.unwrap();
    }

    {
        buf.seek(SeekFrom::Start(0)).unwrap();
        let mut r = lz4::AsyncDecoder::new(&mut buf).unwrap();
        let mut decoded = Vec::new();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            r.read_to_end(&mut decoded).await.unwrap();
            assert_eq!(decoded, data);
        });
    }
    
}
