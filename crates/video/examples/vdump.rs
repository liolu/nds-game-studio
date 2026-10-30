//! Écrit toutes les images en YUV 4:2:0 et le son en f32 : `vdump <fichier> <sortie.yuv> <sortie.f32>`.
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut video = nds_video::Video::open(std::fs::read(&args[1]).unwrap()).unwrap();
    let mut out = std::io::BufWriter::new(std::fs::File::create(&args[2]).unwrap());
    let (mut yuv, mut pcm) = (Vec::new(), Vec::new());
    while video.advance(&mut pcm) {
        video.yuv(&mut yuv);
        out.write_all(&yuv).unwrap();
    }
    std::fs::write(&args[3], pcm.iter().flat_map(|s| s.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
    println!("erreurs {:?}", video.errors());
}
