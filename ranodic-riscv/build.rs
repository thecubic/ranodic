#[macro_export]
macro_rules! assert_unique_used_features {
    ($($feature:literal),+ $(,)?) => {
        assert!(
            (0 $(+ cfg!(feature = $feature) as usize)+ ) == 1,
            "Exactly one of the following features must be enabled: {}",
            [$($feature),+].join(", ")
        );
    };
}

fn main() {
    let target = std::env::var("TARGET").unwrap();
    #[cfg(feature = "esp32c6")]
    {
        assert!(
            target == "riscv32imac-unknown-none-elf",
            "feature esp32c6 does not match target {target}"
        );
        println!("cargo:rustc-cfg=esp32c6");
    }
    linker_be_nice();
    println!("cargo:rustc-link-arg=-Tdefmt.x");
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}

fn linker_be_nice() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() > 1 {
        let kind = &args[1];
        let what = &args[2];

        match kind.as_str() {
            "undefined-symbol" => match what.as_str() {
                "_defmt_write" | "_defmt_release" | "_defmt_acquire" => {
                    eprintln!();
                    eprintln!("💡 some part of `defmt` not found");
                    eprintln!();
                }
                "_defmt_timestamp" => {
                    eprintln!();
                    eprintln!(
                        "💡 `defmt` not found - make sure `defmt.x` is added as a linker script and you have included `use defmt_rtt as _;`"
                    );
                    eprintln!();
                }
                "_stack_start" => {
                    eprintln!();
                    eprintln!("💡 Is the linker script `linkall.x` missing?");
                    eprintln!();
                }
                "esp_wifi_preempt_enable"
                | "esp_wifi_preempt_yield_task"
                | "esp_wifi_preempt_task_create" => {
                    eprintln!();
                    eprintln!(
                        "💡 `esp-wifi` has no scheduler enabled. Make sure you have the `builtin-scheduler` feature enabled, or that you provide an external scheduler."
                    );
                    eprintln!();
                }
                "embedded_test_linker_file_not_added_to_rustflags" => {
                    eprintln!();
                    eprintln!(
                        "💡 `embedded-test` not found - make sure `embedded-test.x` is added as a linker script for tests"
                    );
                    eprintln!();
                }
                _ => (),
            },
            // we don't have anything helpful for "missing-lib" yet
            _ => {
                std::process::exit(1);
            }
        }
        std::process::exit(0);
    }

    println!(
        "cargo:rustc-link-arg=--error-handling-script={}",
        std::env::current_exe().unwrap().display()
    );
}
