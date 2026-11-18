fn main() -> anyhow::Result<()> {
    println!(
        "tomi (format {}..={}, api v{})",
        tomi_core::FORMAT_MIN,
        tomi_core::FORMAT_MAX,
        tomi_core::API_VERSION
    );
    Ok(())
}
