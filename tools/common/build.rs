//! Date et heure de compilation (UTC) au format AA.MM.JJ_HH:MM, pour le numéro de version affiché
//! (ex. « 26.10.01_16:10_v0.9.1.1 »). `NDSGS_DATE` (CI) la remplace si elle est définie.

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    // Algorithme de H. Hinnant : jours depuis 1970-01-01 -> (année, mois, jour)
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn main() {
    println!("cargo:rerun-if-env-changed=NDSGS_DATE");
    let date = std::env::var("NDSGS_DATE").unwrap_or_else(|_| {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
        let minutes = secs.rem_euclid(86_400) / 60;
        format!("{:02}.{:02}.{:02}_{:02}:{:02}", y.rem_euclid(100), m, d, minutes / 60, minutes % 60)
    });
    println!("cargo:rustc-env=NDSGS_BUILD_DATE={date}");
}
