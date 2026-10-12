//! Host-only golden images. Never compare the desktop compositor or wall clock.
use std::{
    fs,
    io::{BufReader, BufWriter},
    path::{Path, PathBuf},
};

pub struct Screenshots {
    baseline: PathBuf,
    artifacts: PathBuf,
    failures: Vec<String>,
}

impl Screenshots {
    pub fn new(manifest: &str) -> Self {
        let project = Path::new(manifest);
        Self {
            baseline: project.join("tests/screenshots"),
            artifacts: project
                .join("../../target/screenshots")
                .join(project.file_name().unwrap()),
            failures: Vec::new(),
        }
    }

    pub fn check(&mut self, name: &str, width: u32, height: u32, rgb: &[u8]) {
        assert_eq!(rgb.len(), width as usize * height as usize * 3);
        let baseline = self.baseline.join(format!("{name}.png"));
        if std::env::var("METALOS_UPDATE_SCREENSHOTS").as_deref() == Ok("1") {
            write_png(&baseline, width, height, rgb);
            return;
        }
        let expected = fs::File::open(&baseline).ok().map(|file| {
            let mut reader = png::Decoder::new(BufReader::new(file)).read_info().unwrap();
            let mut bytes = vec![0; reader.output_buffer_size().unwrap()];
            let info = reader.next_frame(&mut bytes).unwrap();
            assert_eq!(info.color_type, png::ColorType::Rgb);
            assert_eq!(info.bit_depth, png::BitDepth::Eight);
            bytes.truncate(info.buffer_size());
            (info.width, info.height, bytes)
        });
        if expected
            .as_ref()
            .is_some_and(|(w, h, bytes)| *w == width && *h == height && bytes == rgb)
        {
            return;
        }
        let actual = self.artifacts.join(format!("{name}.actual.png"));
        write_png(&actual, width, height, rgb);
        let reason = if let Some((w, h, bytes)) = expected {
            write_png(
                &self.artifacts.join(format!("{name}.expected.png")),
                w,
                h,
                &bytes,
            );
            if w == width && h == height {
                let mut diff = vec![0; rgb.len()];
                let mut changed = 0;
                for ((a, b), d) in rgb
                    .chunks_exact(3)
                    .zip(bytes.chunks_exact(3))
                    .zip(diff.chunks_exact_mut(3))
                {
                    if a != b {
                        changed += 1;
                        d.copy_from_slice(&[255, 0, 128]);
                    }
                }
                write_png(
                    &self.artifacts.join(format!("{name}.diff.png")),
                    width,
                    height,
                    &diff,
                );
                format!("{changed} pixels changed")
            } else {
                format!("dimensions changed from {w}x{h} to {width}x{height}")
            }
        } else {
            "missing baseline".into()
        };
        self.failures
            .push(format!("{name}: {reason}; actual: {}", actual.display()));
    }

    pub fn finish(self) {
        assert!(
            self.failures.is_empty(),
            "Screenshot regressions:\n{}\nReview artifacts before explicitly updating baselines.",
            self.failures.join("\n")
        );
    }
}

fn write_png(path: &Path, width: u32, height: u32, rgb: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut encoder = png::Encoder::new(
        BufWriter::new(fs::File::create(path).unwrap()),
        width,
        height,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(rgb)
        .unwrap();
}
