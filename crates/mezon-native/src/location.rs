pub fn current_location() -> anyhow::Result<(f64, f64)> {
    ip_geolocate()
}

fn ip_geolocate() -> anyhow::Result<(f64, f64)> {
    let output = std::process::Command::new("curl")
        .args(["-sS", "--max-time", "5", "https://ipinfo.io/loc"])
        .output()?;
    if !output.status.success() {
        return Err(anyhow::anyhow!("ip geolocation failed"));
    }
    parse_coords(String::from_utf8_lossy(&output.stdout).trim())
}

fn parse_coords(raw: &str) -> anyhow::Result<(f64, f64)> {
    let mut parts = raw.split(',');
    let lat = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing latitude"))?
        .trim()
        .parse::<f64>()?;
    let lng = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("missing longitude"))?
        .trim()
        .parse::<f64>()?;
    if !lat.is_finite() || !lng.is_finite() {
        return Err(anyhow::anyhow!("invalid coordinates"));
    }
    Ok((lat, lng))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_coords_reads_lat_lng_pair() {
        let (lat, lng) = parse_coords("10.5,106.2").expect("coords");
        assert!((lat - 10.5).abs() < f64::EPSILON);
        assert!((lng - 106.2).abs() < f64::EPSILON);
    }
}
