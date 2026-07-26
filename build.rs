fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    compile_resource();
}

#[cfg(not(target_env = "msvc"))]
fn compile_resource() {}

#[cfg(target_env = "msvc")]
fn compile_resource() {
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let icon = output.join("codex-status.ico");
    let manifest = output.join("codex-status.manifest");
    let resource = output.join("codex-status.rc");
    std::fs::write(&icon, make_icon()).expect("write generated icon");
    std::fs::write(
        &manifest,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity version="0.1.0.0" processorArchitecture="amd64" name="CodexStatus" type="win32" />
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false" /></requestedPrivileges></security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{4f476546-937a-4f71-9680-68e4d9c13cf8}" />
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}" />
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2,PerMonitor</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>
"#,
    )
    .expect("write generated manifest");
    let icon_path = escape_resource_path(&icon);
    let manifest_path = escape_resource_path(&manifest);
    let rc = format!(
        "1 ICON \"{icon_path}\"\n1 24 \"{manifest_path}\"\n1 VERSIONINFO\nFILEVERSION 0,1,0,0\nPRODUCTVERSION 0,1,0,0\nBEGIN\n BLOCK \"StringFileInfo\"\n BEGIN\n  BLOCK \"040904E4\"\n  BEGIN\n   VALUE \"CompanyName\", \"CodexStatus Contributors\\0\"\n   VALUE \"FileDescription\", \"Codex weekly quota in the Windows tray\\0\"\n   VALUE \"FileVersion\", \"0.1.0\\0\"\n   VALUE \"InternalName\", \"CodexStatus\\0\"\n   VALUE \"OriginalFilename\", \"CodexStatus.exe\\0\"\n   VALUE \"ProductName\", \"CodexStatus\\0\"\n   VALUE \"ProductVersion\", \"0.1.0\\0\"\n  END\n END\n BLOCK \"VarFileInfo\"\n BEGIN\n  VALUE \"Translation\", 0x0409, 1252\n END\nEND\n"
    );
    std::fs::write(&resource, rc).expect("write generated resource");
    embed_resource::compile(resource, embed_resource::NONE).manifest_required().unwrap();
}

#[cfg(target_env = "msvc")]
fn escape_resource_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "\\\\")
}

#[cfg(target_env = "msvc")]
fn make_icon() -> Vec<u8> {
    let sizes = [16_u32, 32, 48, 64];
    let images: Vec<_> = sizes.into_iter().map(|size| (size, icon_dib(size))).collect();
    let directory_size = 6 + images.len() * 16;
    let mut output = Vec::new();
    output.extend_from_slice(&0_u16.to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = directory_size as u32;
    for (size, bytes) in &images {
        output.push(*size as u8);
        output.push(*size as u8);
        output.push(0);
        output.push(0);
        output.extend_from_slice(&1_u16.to_le_bytes());
        output.extend_from_slice(&32_u16.to_le_bytes());
        output.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        output.extend_from_slice(&offset.to_le_bytes());
        offset += bytes.len() as u32;
    }
    for (_, bytes) in images {
        output.extend_from_slice(&bytes);
    }
    output
}

#[cfg(target_env = "msvc")]
fn icon_dib(size: u32) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&40_u32.to_le_bytes());
    output.extend_from_slice(&(size as i32).to_le_bytes());
    output.extend_from_slice(&((size * 2) as i32).to_le_bytes());
    output.extend_from_slice(&1_u16.to_le_bytes());
    output.extend_from_slice(&32_u16.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&(size * size * 4).to_le_bytes());
    output.extend_from_slice(&0_i32.to_le_bytes());
    output.extend_from_slice(&0_i32.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    output.extend_from_slice(&0_u32.to_le_bytes());
    for y in (0..size).rev() {
        for x in 0..size {
            let cx = x as f32 - (size as f32 - 1.0) / 2.0;
            let cy = y as f32 - (size as f32 - 1.0) / 2.0;
            let edge_x = x.min(size - 1 - x) as f32;
            let edge_y = y.min(size - 1 - y) as f32;
            let corner = size as f32 * 0.2;
            let inside = edge_x >= corner || edge_y >= corner || {
                let dx = corner - edge_x - 0.5;
                let dy = corner - edge_y - 0.5;
                dx * dx + dy * dy <= corner * corner
            };
            let distance = (cx * cx + cy * cy).sqrt();
            let letter = distance >= size as f32 * 0.20
                && distance <= size as f32 * 0.32
                && !(cx > size as f32 * 0.02 && cy.abs() < size as f32 * 0.16);
            let (b, g, r, a) = if !inside {
                (0, 0, 0, 0)
            } else if letter {
                (255, 255, 255, 255)
            } else {
                (110, 159, 14, 255)
            };
            output.extend_from_slice(&[b, g, r, a]);
        }
    }
    let mask_stride = size.div_ceil(32) * 4;
    output.resize(output.len() + (mask_stride * size) as usize, 0);
    output
}
