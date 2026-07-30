//! Host-only raw-f32 probe for the bundled DeepFilterNet 3 backend.
//!
//! This tool deliberately has no device or NPU transport. It exists to
//! qualify the CPU reference used by the learned Erf residual pipeline.

use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use waved_npu::{Dfn3CpuInfer, NpuInfer};

struct Arguments {
    input: PathBuf,
    output: PathBuf,
    attenuation_limit_db: f32,
}

fn arguments() -> Result<Arguments> {
    let mut values = env::args_os().skip(1);
    let mut input = None;
    let mut output = None;
    let mut attenuation_limit_db = None;
    while let Some(flag) = values.next() {
        match flag.to_str() {
            Some("--input") if input.is_none() => {
                input = values.next().map(PathBuf::from);
            }
            Some("--output") if output.is_none() => {
                output = values.next().map(PathBuf::from);
            }
            Some("--attenuation-limit-db") if attenuation_limit_db.is_none() => {
                let raw = values
                    .next()
                    .context("--attenuation-limit-db requires a value")?;
                attenuation_limit_db = Some(
                    raw.to_str()
                        .context("attenuation limit must be UTF-8")?
                        .parse::<f32>()
                        .context("attenuation limit must be numeric")?,
                );
            }
            _ => bail!("unknown, duplicate, or incomplete argument"),
        }
    }
    let attenuation_limit_db =
        attenuation_limit_db.context("--attenuation-limit-db is required")?;
    if !attenuation_limit_db.is_finite() || !(0.0..=100.0).contains(&attenuation_limit_db) {
        bail!("attenuation limit must be finite and between 0 and 100 dB");
    }
    Ok(Arguments {
        input: input.context("--input is required")?,
        output: output.context("--output is required")?,
        attenuation_limit_db,
    })
}

fn input_sample_count(path: &Path, hop_size: usize) -> Result<usize> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("cannot inspect {}", path.display()))?;
    if !metadata.file_type().is_file() {
        bail!("input must be a regular file");
    }
    let byte_count = usize::try_from(metadata.len()).context("input is too large")?;
    if byte_count == 0 || byte_count % size_of::<f32>() != 0 {
        bail!("input byte count must be a nonzero multiple of four");
    }
    let sample_count = byte_count / size_of::<f32>();
    if sample_count % hop_size != 0 {
        bail!(
            "input sample count {} is not a multiple of hop size {}",
            sample_count,
            hop_size
        );
    }
    Ok(sample_count)
}

fn process_stream(
    input_path: &Path,
    output_path: &Path,
    model: &mut Dfn3CpuInfer,
    sample_count: usize,
) -> Result<()> {
    let parent = output_path.parent().context("output must have a parent")?;
    fs::create_dir_all(parent).with_context(|| format!("cannot create {}", parent.display()))?;
    let input =
        File::open(input_path).with_context(|| format!("cannot open {}", input_path.display()))?;
    let output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)
        .with_context(|| format!("cannot exclusively create {}", output_path.display()))?;
    let result = (|| -> Result<()> {
        let mut reader = BufReader::with_capacity(model.hop_size() * size_of::<f32>() * 16, input);
        let mut writer = BufWriter::with_capacity(model.hop_size() * size_of::<f32>() * 16, output);
        let mut input_bytes = vec![0_u8; model.hop_size() * size_of::<f32>()];
        let mut input_hop = vec![0_f32; model.hop_size()];
        for _ in 0..sample_count / model.hop_size() {
            reader
                .read_exact(&mut input_bytes)
                .context("cannot read complete input hop")?;
            for (sample, bytes) in input_hop
                .iter_mut()
                .zip(input_bytes.chunks_exact(size_of::<f32>()))
            {
                *sample = f32::from_le_bytes(bytes.try_into().expect("four-byte chunk"));
                if !sample.is_finite() {
                    bail!("input contains a non-finite sample");
                }
            }
            let enhanced = model.run(&input_hop)?;
            if enhanced.len() != model.hop_size() {
                bail!("model output hop length differs");
            }
            for sample in enhanced {
                if !sample.is_finite() {
                    bail!("model produced a non-finite sample");
                }
                writer
                    .write_all(&sample.to_le_bytes())
                    .context("cannot write output hop")?;
            }
        }
        let mut trailing = [0_u8; 1];
        if reader
            .read(&mut trailing)
            .context("cannot check input EOF")?
            != 0
        {
            bail!("input grew while processing");
        }
        writer.flush().context("cannot flush output")?;
        writer.get_ref().sync_all().context("cannot sync output")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(output_path);
    }
    result
}

fn run() -> Result<()> {
    let args = arguments()?;
    let mut model = Dfn3CpuInfer::with_atten_lim_db(args.attenuation_limit_db)?;
    let sample_count = input_sample_count(&args.input, model.hop_size())?;
    process_stream(&args.input, &args.output, &mut model, sample_count)?;
    println!(
        "{{\"attenuation_limit_db\":{},\"backend\":\"{}\",\
         \"hop_samples\":{},\"samples\":{}}}",
        args.attenuation_limit_db,
        model.provider(),
        model.hop_size(),
        sample_count
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("dfn3_host_filter: {error:#}");
        std::process::exit(2);
    }
}
