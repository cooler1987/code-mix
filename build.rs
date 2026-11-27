use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn append_ico_image(out: &mut Vec<u8>, size: u32) {
    // BITMAPINFOHEADER (40 bytes)
    out.extend_from_slice(&40u32.to_le_bytes()); // biSize
    out.extend_from_slice(&(size as i32).to_le_bytes()); // biWidth
    out.extend_from_slice(&((size * 2) as i32).to_le_bytes()); // biHeight (color + AND mask)
    out.extend_from_slice(&1u16.to_le_bytes()); // biPlanes
    out.extend_from_slice(&32u16.to_le_bytes()); // biBitCount
    out.extend_from_slice(&0u32.to_le_bytes()); // biCompression = BI_RGB
    out.extend_from_slice(&(size * size * 4).to_le_bytes()); // biSizeImage
    out.extend_from_slice(&0i32.to_le_bytes()); // biXPelsPerMeter
    out.extend_from_slice(&0i32.to_le_bytes()); // biYPelsPerMeter
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrUsed
    out.extend_from_slice(&0u32.to_le_bytes()); // biClrImportant

    let s = size as f32;
    let margin = s * 0.06;
    let radius = s * 0.22;

    // DIB pixel rows are bottom-up (y from size - 1 down to 0)
    for y_inv in 0..size {
        let py = (size - 1 - y_inv) as f32 + 0.5;
        for x in 0..size {
            let px = x as f32 + 0.5;

            // Signed distance to rounded square
            let half = s * 0.5 - margin - radius;
            let cx = (px - s * 0.5).abs() - half;
            let cy = (py - s * 0.5).abs() - half;
            let dist = (cx.max(0.0).powi(2) + cy.max(0.0).powi(2)).sqrt() + cx.max(cy).min(0.0) - radius;

            let alpha_cov = (0.5 - dist).clamp(0.0, 1.0);
            if alpha_cov <= 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }

            // Blue-indigo diagonal gradient background
            let t = ((px + py) / (2.0 * s)).clamp(0.0, 1.0);
            let mut r = 37.0 + (29.0 - 37.0) * t;
            let mut g = 99.0 + (78.0 - 99.0) * t;
            let mut b = 235.0 + (216.0 - 235.0) * t;

            // Stylized white "< >" / code box motif in normalized coordinates
            let nx = px / s;
            let ny = py / s;
            let stroke = 0.085;

            // Left chevron "<"
            let left_top = ((ny - 0.5) - (nx - 0.24) * 1.15).abs() < stroke
                && nx >= 0.22
                && nx <= 0.44
                && ny >= 0.28
                && ny <= 0.52;
            let left_bot = ((ny - 0.5) + (nx - 0.24) * 1.15).abs() < stroke
                && nx >= 0.22
                && nx <= 0.44
                && ny >= 0.48
                && ny <= 0.72;

            // Right chevron ">"
            let right_top = ((ny - 0.5) + (nx - 0.76) * 1.15).abs() < stroke
                && nx >= 0.56
                && nx <= 0.78
                && ny >= 0.28
                && ny <= 0.52;
            let right_bot = ((ny - 0.5) - (nx - 0.76) * 1.15).abs() < stroke
                && nx >= 0.56
                && nx <= 0.78
                && ny >= 0.48
                && ny <= 0.72;

            // Center slash "/"
            let slash = ((nx - 0.5) + (ny - 0.5) * 0.35).abs() < stroke * 0.65
                && ny >= 0.28
                && ny <= 0.72;

            if left_top || left_bot || right_top || right_bot || slash {
                r = 255.0;
                g = 255.0;
                b = 255.0;
            }

            let a = (alpha_cov * 255.0).round() as u8;
            out.extend_from_slice(&[b as u8, g as u8, r as u8, a]);
        }
    }

    // AND mask (1bpp, rows padded to 4 bytes)
    let mask_row_bytes = ((size + 31) / 32) * 4;
    out.resize(out.len() + (mask_row_bytes * size) as usize, 0);
}

fn generate_ico_bytes() -> Vec<u8> {
    let sizes = [16u32, 32u32, 48u32];
    let mut images: Vec<Vec<u8>> = Vec::with_capacity(sizes.len());
    for &sz in &sizes {
        let mut buf = Vec::new();
        append_ico_image(&mut buf, sz);
        images.push(buf);
    }

    let mut ico = Vec::new();
    // ICONDIR
    ico.extend_from_slice(&0u16.to_le_bytes()); // reserved
    ico.extend_from_slice(&1u16.to_le_bytes()); // type = icon
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes()); // count

    let mut offset = 6u32 + (16u32 * sizes.len() as u32);
    for (idx, &sz) in sizes.iter().enumerate() {
        let dim = if sz >= 256 { 0u8 } else { sz as u8 };
        let byte_len = images[idx].len() as u32;
        ico.push(dim); // bWidth
        ico.push(dim); // bHeight
        ico.push(0); // bColorCount
        ico.push(0); // bReserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // wPlanes
        ico.extend_from_slice(&32u16.to_le_bytes()); // wBitCount
        ico.extend_from_slice(&byte_len.to_le_bytes()); // dwBytesInRes
        ico.extend_from_slice(&offset.to_le_bytes()); // dwImageOffset
        offset += byte_len;
    }

    for img in images {
        ico.extend_from_slice(&img);
    }
    ico
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    if env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR missing"));
    let ico_path = out_dir.join("app.ico");
    if let Ok(mut f) = File::create(&ico_path) {
        let _ = f.write_all(&generate_ico_bytes());
    }

    let manifest = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="1.1.0.0" processorArchitecture="*" name="Repomix.Portable" type="win32"/>
  <description>Repomix Portable Codebase Packer</description>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true</dpiAware>
    </windowsSettings>
  </application>
</assembly>"#;

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico_path.to_string_lossy().as_ref());
    res.set_manifest(manifest);
    res.set("ProductName", "Repomix Portable");
    res.set("FileDescription", "Repomix Portable - Codebase Packer & Restore Tool");
    res.set("OriginalFilename", "repomix-portable.exe");

    if let Err(e) = res.compile() {
        println!("cargo:warning=winresource compile skipped: {e}");
    }
}
