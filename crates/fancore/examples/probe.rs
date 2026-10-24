//! Read-only hardware probe: prints fans, every temperature key and each
//! sensor group. Useful when porting fanctl to a new Mac.
//!
//! ```sh
//! cargo run -p fancore --example probe
//! ```

use fancore::smc::Smc;
use fancore::types::SensorGroup;

fn main() {
    let smc = match Smc::open() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("fan count: {}", smc.fan_count());
    for fan in smc.read_fans() {
        println!(
            "fan {}: {:.0} rpm (target {:.0}, {:.0}–{:.0}, {})",
            fan.index,
            fan.actual_rpm,
            fan.target_rpm,
            fan.min_rpm,
            fan.max_rpm,
            fan.mode.label()
        );
    }

    let all = smc.all_keys();
    println!("\nSMC keys: {}", all.len());
    let temp_keys: Vec<String> = all
        .iter()
        .filter(|k| k.starts_with('T') && k.len() == 4)
        .filter(|k| {
            smc.key_info(k)
                .map(|(size, ty)| {
                    (size == 4 && ty == "flt ") || (size == 2 && ty.starts_with("sp"))
                })
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    let readings = smc.read_temperatures(&temp_keys);
    println!("readable temperature keys: {}", readings.len());

    for group in SensorGroup::ALL {
        let keys = group.keys(&all);
        if keys.is_empty() {
            continue;
        }
        let value = fancore::group_temp(&keys, &readings);
        println!("\n[{}] {:.1?} °C  keys={}", group.label(), value, keys.join(","));
    }

    println!("\nraw readings:");
    let mut sorted = readings.clone();
    sorted.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (k, v) in sorted {
        println!("  {k}  {v:.2}");
    }
}
