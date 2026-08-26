use voidx_client::{list, Device};
use voidx_proto::NodePath;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let found = list()?;
    if found.is_empty() {
        return Err("no USB port identified as a StompStation PRO".into());
    }
    for candidate in &found {
        println!(
            "found {}: {} / {} ({:04x}:{:04x})",
            candidate.port_name,
            candidate.manufacturer.as_deref().unwrap_or("unknown maker"),
            candidate.product.as_deref().unwrap_or("unknown product"),
            candidate.vendor_id.unwrap_or(0),
            candidate.product_id.unwrap_or(0),
        );
    }

    let mut device = Device::connect(found[0].open()?)?;
    println!("identity: {:?}", device.identity());
    println!("write safety: {:?}", device.write_safety());

    let app = device.browse(NodePath::new("root\\app")?)?;
    println!("app schema: {} nodes", app.nodes().len());
    for path in [
        "root\\presets",
        "root\\ir_list",
        "root\\nam_amp",
        "root\\nam_drive",
    ] {
        let list = device.list_info(NodePath::new(path)?)?;
        println!(
            "{}: {}/{} occupied, {} bytes in {}-byte chunks, type {:?}",
            path,
            list.occupied().count(),
            list.count,
            list.size,
            list.chunk_size,
            list.item_type,
        );
        if let Some((index, name)) = list.occupied().next() {
            let first = device.read_blob_chunk(&list.path, index, 1)?;
            println!(
                "  slot {} {:?}: read {} bytes from chunk 1",
                index + 1,
                name,
                first.len()
            );
            if path == "root\\presets" {
                let started = std::time::Instant::now();
                let blob = device.read_blob(&list, index)?;
                println!(
                    "  full preset: {} bytes in {:.2?}",
                    blob.len(),
                    started.elapsed()
                );
            }
        };
    }
    Ok(())
}
