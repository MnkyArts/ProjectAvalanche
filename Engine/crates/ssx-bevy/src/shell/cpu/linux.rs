use super::Cores;

#[repr(C, align(8))]
struct SchedAttr([u8; SCHED_ATTR_SIZE_VER1 as usize]);

const SCHED_ATTR_SIZE_VER1: u32 = 56;

impl SchedAttr {
    fn new() -> SchedAttr {
        let mut a = SchedAttr([0; SCHED_ATTR_SIZE_VER1 as usize]);
        a.0[0..4].copy_from_slice(&SCHED_ATTR_SIZE_VER1.to_ne_bytes());
        a
    }

    fn util_min_request(min: u32) -> SchedAttr {
        let mut a = SchedAttr::new();
        let flags = (libc::SCHED_FLAG_KEEP_ALL | libc::SCHED_FLAG_UTIL_CLAMP_MIN) as u64;
        a.0[8..16].copy_from_slice(&flags.to_ne_bytes());
        a.0[48..52].copy_from_slice(&min.to_ne_bytes());
        a
    }

    fn util_min(&self) -> u32 {
        u32::from_ne_bytes([self.0[48], self.0[49], self.0[50], self.0[51]])
    }
}

pub(super) fn place_threads() -> String {
    let cores = match cores() {
        Ok(c) => c,
        Err(why) => return format!("cpu: the OS places the game's threads ({why})"),
    };
    let Some(floor) = cores.floor() else {
        return format!("cpu: {} cores of one capacity: the OS places the game's threads", cores.0.len());
    };
    let mut now = SchedAttr::new();
    let (this, no_flags): (libc::c_long, libc::c_long) = (0, 0);
    let size = libc::c_long::from(SCHED_ATTR_SIZE_VER1);
    let got = unsafe { libc::syscall(libc::SYS_sched_getattr, this, now.0.as_mut_ptr(), size, no_flags) };
    if got == 0 && now.util_min() > 0 {
        return format!(
            "cpu: cores {}; the process already asks for a utilisation of at least {} (kept)",
            cores.classes(),
            now.util_min()
        );
    }
    let attr = SchedAttr::util_min_request(floor);
    let set = unsafe { libc::syscall(libc::SYS_sched_setattr, this, attr.0.as_ptr(), no_flags) };
    if set != 0 {
        let e = std::io::Error::last_os_error();
        return format!(
            "cpu: cores {}; the utilisation clamp was refused ({e}): the OS places the game's threads",
            cores.classes()
        );
    }
    format!(
        "cpu: cores {}; the game's threads ask for a utilisation of at least {floor}: on the fastest cores while one \
         is free (the slower: {})",
        cores.classes(),
        cores.slower()
    )
}

fn cores() -> Result<Cores, String> {
    let mut set: libc::cpu_set_t = unsafe { std::mem::zeroed() };
    if unsafe { libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) } != 0 {
        return Err(format!("the affinity is not known: {}", std::io::Error::last_os_error()));
    }
    let mut cores = Vec::new();
    for cpu in 0..libc::CPU_SETSIZE as usize {
        if !unsafe { libc::CPU_ISSET(cpu, &set) } {
            continue;
        }
        let path = format!("/sys/devices/system/cpu/cpu{cpu}/cpu_capacity");
        let text = std::fs::read_to_string(&path).map_err(|e| format!("no capacity for core {cpu} ({path}: {e})"))?;
        let capacity: u32 = text.trim().parse().map_err(|_| format!("{path}: not a number"))?;
        if capacity == 0 || capacity > 1024 {
            return Err(format!("{path}: {capacity} is outside 1 … 1024"));
        }
        cores.push((cpu as u32, capacity));
    }
    Ok(Cores(cores))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_sets_the_minimum_clamp_alone() {
        let a = SchedAttr::util_min_request(313);
        assert_eq!(u32::from_ne_bytes(a.0[0..4].try_into().unwrap()), 56);
        assert_eq!(u64::from_ne_bytes(a.0[8..16].try_into().unwrap()), 0x08 | 0x10 | 0x20);
        assert_eq!(a.util_min(), 313);
        assert!(a.0[16..48].iter().chain(&a.0[52..]).all(|&b| b == 0));
    }
}
