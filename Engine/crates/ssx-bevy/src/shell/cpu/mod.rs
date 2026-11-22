#[cfg(target_os = "linux")]
mod linux;

pub const SWITCH: &str = "SSX_CPU_PLACEMENT";

pub fn place_threads() -> String {
    if std::env::var(SWITCH).is_ok_and(|v| v.eq_ignore_ascii_case("off")) {
        return format!("cpu: the OS places the game's threads ({SWITCH}=off)");
    }
    #[cfg(target_os = "linux")]
    {
        // PORT(choice): on Linux the process asks for a minimum utilisation just above the second-fastest core class's
        // capacity (`sched_setattr`, `SCHED_FLAG_UTIL_CLAMP_MIN`), so the scheduler places the game's threads on the
        // fastest cores while one is free and on the others only when those are busy. Measured on the development
        // laptop (Core Ultra 7 255H: 6 cores of capacity 1024, 8 of 774, 2 of 312; the benchmark's alps, whitehorn,
        // eddie and deathzone scenes, 30 s each, the mean of two runs): frames over 20 ms 113 / 135 / 347 / 92 without
        // a clamp, 27 / 32 / 20 / 16 with the fastest class alone fitting (a clamp of 1024), 70 / 113 / 125 / 40 with
        // only the 312 cores excluded (512, one run), 73 / 103 / 80 / 57 with the 312 cores taken away (`taskset
        // 0-13`, one run); p99 22.8 / 25.0 / 26.5 / 22.4 → 20.0 / 21.0 / 19.8 / 18.7 ms, 1 % lows 40.0 / 36.8 / 34.7 /
        // 41.3 → 42.7 / 42.5 / 46.0 / 49.1 fps. Under `intel_pstate`'s HWP the clamp does not move the clock;
        // under `schedutil` a running thread asks for at least the clamp's share of the fastest core's clock.
        // `SSX_CPU_PLACEMENT=off` leaves the placement to the OS. The console's six hardware threads were alike.
        linux::place_threads()
    }
    // PORT(choice): no request on Windows and macOS. Windows' scheduler gives the foreground window's process the high
    // quality-of-service class and places it on the fast cores of a hybrid CPU with the processor's hints (Intel
    // Thread Director); macOS places threads by their QoS class (the main thread's user-interactive); Linux's
    // scheduler has no foreground notion. The console's six hardware threads were alike.
    #[cfg(not(target_os = "linux"))]
    {
        "cpu: the OS places the game's threads".to_string()
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cores(pub Vec<(u32, u32)>);

#[cfg(target_os = "linux")]
impl Cores {
    pub fn floor(&self) -> Option<u32> {
        let fastest = self.0.iter().map(|&(_, c)| c).max()?;
        let next = self.0.iter().map(|&(_, c)| c).filter(|&c| c < fastest).max()?;
        Some(next + 1)
    }

    pub fn slower(&self) -> String {
        let Some(fastest) = self.0.iter().map(|&(_, c)| c).max() else { return String::new() };
        let cpus: Vec<u32> = self.0.iter().filter(|&&(_, c)| c < fastest).map(|&(n, _)| n).collect();
        ranges(&cpus)
    }

    pub fn classes(&self) -> String {
        let mut caps: Vec<u32> = self.0.iter().map(|&(_, c)| c).collect();
        caps.sort_unstable_by(|a, b| b.cmp(a));
        caps.dedup();
        let list: Vec<String> =
            caps.iter().map(|c| format!("{} × {c}", self.0.iter().filter(|&&(_, k)| k == *c).count())).collect();
        list.join(", ")
    }
}

#[cfg(target_os = "linux")]
fn ranges(cpus: &[u32]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < cpus.len() {
        let mut j = i;
        while j + 1 < cpus.len() && cpus[j + 1] == cpus[j] + 1 {
            j += 1;
        }
        out.push(if i == j { cpus[i].to_string() } else { format!("{}-{}", cpus[i], cpus[j]) });
        i = j + 1;
    }
    out.join(",")
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn floor_only_for_cores_that_differ() {
        assert_eq!(Cores(vec![(0, 1024), (1, 1024)]).floor(), None);
        assert_eq!(Cores(Vec::new()).floor(), None);
        let hybrid = Cores(
            (0..16)
                .map(|n| {
                    (
                        n,
                        if n < 6 {
                            1024
                        } else if n < 14 {
                            774
                        } else {
                            312
                        },
                    )
                })
                .collect(),
        );
        assert_eq!(hybrid.floor(), Some(775));
        assert_eq!(hybrid.slower(), "6-15");
        assert_eq!(hybrid.classes(), "6 × 1024, 8 × 774, 2 × 312");
    }

    #[test]
    fn core_lists() {
        assert_eq!(ranges(&[0, 1, 2, 5, 7, 8]), "0-2,5,7-8");
        assert_eq!(ranges(&[]), "");
    }
}
