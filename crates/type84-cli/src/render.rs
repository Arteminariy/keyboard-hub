//! Human-readable output.

use std::fmt::Write as _;

use type84_proto::device_info::DeviceInfo;
use type84_proto::dks;
use type84_proto::keymap::{self, KeyAction};
use type84_proto::layout;
use type84_proto::led::{self, LedEffect};
use type84_proto::rt::{self, AxisRecord, RtScale};
use type84_proto::settings::{self, Settings};

use crate::snapshot::Snapshot;

fn on_off(v: u8) -> String {
    match v {
        0 => "off".into(),
        1 => "on".into(),
        n => format!("raw {n}"),
    }
}

pub fn device_info(info: &DeviceInfo) -> String {
    let mut s = String::new();
    let scale = RtScale::from_precision(info.rt_precision);
    let _ = writeln!(s, "Firmware version   {}", info.version_string());
    let _ = writeln!(
        s,
        "VID:PID            {:04x}:{:04x}",
        info.vendor_id, info.product_id
    );
    let _ = writeln!(s, "Manufacturer code  {:#06x}", info.manufacturer);
    let _ = writeln!(s, "Product code       {:#06x}", info.product);
    let _ = writeln!(s, "Work mode          {:#04x}", info.work_mode);
    let _ = writeln!(
        s,
        "Axis info          {} ({})",
        info.axis_info,
        if info.is_magnetic() {
            "magnetic"
        } else {
            "not magnetic"
        }
    );
    let _ = writeln!(
        s,
        "RT precision       {} (actuation step {} mm, RT step {} mm)",
        info.rt_precision,
        1.0 / scale.trigger as f64,
        1.0 / scale.sensitivity as f64
    );
    let _ = writeln!(s, "Frame version      {}", info.frame_version);
    let _ = writeln!(s, "Lighting version   {}", info.lighting_version);
    let _ = writeln!(s, "Current profile    {}", info.current_profile);
    let _ = writeln!(s, "ROM size byte      {}", info.rom_size);
    let _ = writeln!(
        s,
        "Macro space (raw)  {:#06x} (ignored by vendor hub)",
        info.macro_space_size_raw
    );
    let _ = writeln!(
        s,
        "Firmware status    {} ({})",
        info.firmware_status,
        if info.is_operational() {
            "operational"
        } else {
            "BUSY"
        }
    );
    s
}

pub fn settings(raw: &[u8]) -> String {
    let mut s = String::new();
    let Ok(st) = Settings::decode(raw) else {
        return "settings block too short\n".into();
    };
    let rate = st
        .report_rate_hz
        .map(|h| format!("{h} Hz"))
        .unwrap_or_else(|| "unknown".into());
    let _ = writeln!(
        s,
        "Report rate        {rate} (code {})",
        st.report_rate_code
    );
    let _ = writeln!(s, "Auto calibration   {}", on_off(st.auto_calibration));
    let _ = writeln!(s, "Stability mode     {}", on_off(st.stability_mode));
    let dz = if st.top_dead_zone == 0 && st.bottom_dead_zone == 0 {
        "off"
    } else {
        "on"
    };
    let _ = writeln!(
        s,
        "Dead zones         {dz} (top {:.2} mm, bottom {:.2} mm)",
        st.top_dead_zone_mm(),
        st.bottom_dead_zone_mm()
    );
    let _ = writeln!(s, "-- not shown by the vendor hub for this board --");
    let _ = writeln!(s, "Sleep time         {} min", st.sleep_time_min);
    let _ = writeln!(s, "Key delay          {}", st.key_delay);
    let _ = writeln!(s, "Game mode (b1)     {}", st.game_mode);
    let _ = writeln!(s, "Fn switch (b2)     {}", st.fn_switch);
    let _ = writeln!(s, "System mode (b6)   {}", st.system_mode);
    let _ = writeln!(s, "Single-key wakeup  {}", st.single_key_wakeup);
    let _ = writeln!(s, "Push-button mode   {}", st.push_button_mode);
    let _ = writeln!(s, "NKRO switch        {}", st.nkro_switch);
    let unknown: Vec<String> = settings::unknown_offsets()
        .filter(|&i| raw[i] != 0)
        .map(|i| format!("[{i}]={:#04x}", raw[i]))
        .collect();
    let _ = writeln!(
        s,
        "Unmodelled bytes   {}",
        if unknown.is_empty() {
            "all zero".to_string()
        } else {
            unknown.join(" ")
        }
    );
    let _ = writeln!(s, "Raw                {}", hex::encode(raw));
    s
}

pub fn lighting(raw: &[u8]) -> String {
    let Some(e) = LedEffect::decode(raw) else {
        return "LED effect block too short\n".into();
    };
    let mut s = String::new();
    let _ = writeln!(s, "Effect             {} (mode {})", e.mode_name(), e.mode);
    let _ = writeln!(
        s,
        "Colour             #{:02X}{:02X}{:02X}",
        e.color[0], e.color[1], e.color[2]
    );
    let _ = writeln!(
        s,
        "Secondary colour   #{:02X}{:02X}{:02X}",
        e.secondary_color[0], e.secondary_color[1], e.secondary_color[2]
    );
    let _ = writeln!(s, "Colour mode        {}", e.color_mode);
    let _ = writeln!(s, "Brightness         {}/5", e.brightness);
    let _ = writeln!(s, "Speed              {}/5", e.speed);
    let _ = writeln!(s, "Direction          {}", e.direction);
    let _ = writeln!(s, "Raw                {}", hex::encode(raw));
    s
}

fn fmt_mm(v: f64) -> String {
    format!("{v:.2}")
}

pub fn key_row(rec: &AxisRecord, scale: RtScale) -> (String, String, String, String) {
    let act = fmt_mm(rec.actuation_mm(scale));
    if rec.rt_enabled() {
        (
            "on".into(),
            act,
            fmt_mm(rec.press_mm(scale)),
            fmt_mm(rec.release_mm(scale)),
        )
    } else {
        ("off".into(), act, "-".into(), "-".into())
    }
}

pub fn dump(snap: &Snapshot) -> String {
    let mut s = String::new();
    let scale = RtScale::from_precision(snap.info.rt_precision);
    let rt = rt::decode_block(&snap.axis_rt.data);
    let l0 = keymap::decode_block(&snap.keymap_layer0.data);
    let l1 = keymap::decode_block(&snap.keymap_layer1.data);

    let _ = writeln!(s, "== Device ==");
    s.push_str(&device_info(&snap.info));
    let _ = writeln!(s, "\n== Keyboard-wide settings ==");
    s.push_str(&settings(&snap.settings.data));
    let _ = writeln!(s, "\n== Lighting ==");
    s.push_str(&lighting(&snap.led_effect.data));
    let colored = led::decode_custom(&snap.custom_led.data)
        .iter()
        .filter(|c| c.rgb != [0, 0, 0])
        .count();
    let _ = writeln!(s, "Per-key colours    {colored} non-black records");

    let _ = writeln!(s, "\n== Per-key (actuation and Rapid Trigger in mm) ==");
    let _ = writeln!(
        s,
        "{:>3}  {:<10} {:>5}  {:<3} {:>5} {:>7}  {:<5} {:<7} {:<5} {:<4}  {:<14} Fn layer",
        "idx",
        "key",
        "act",
        "RT",
        "press",
        "release",
        "whole",
        "rampage",
        "flags",
        "axis",
        "layer 0"
    );
    for k in layout::keys() {
        let i = k.index as usize;
        let rec = &rt[i];
        let (rt_on, act, press, rel) = key_row(rec, scale);
        let _ = writeln!(
            s,
            "{:>3}  {:<10} {:>5}  {:<3} {:>5} {:>7}  {:<5} {:<7} {:<5} {:<4}  {:<14} {}",
            i,
            k.label,
            act,
            rt_on,
            press,
            rel,
            if rec.whole_travel() { "on" } else { "off" },
            if rec.rampage() { "on" } else { "off" },
            format!("{:#04x}", rec.flags),
            rec.axis_type,
            l0[i].describe(),
            l1[i].describe()
        );
    }

    let extra: Vec<String> = (0..128)
        .filter(|&i| !layout::is_physical(i))
        .filter(|&i| {
            rt[i].encode() != [0; 8] || l0[i] != KeyAction::None || l1[i] != KeyAction::None
        })
        .map(|i| {
            format!(
                "  idx {i:>3}: rt {} | layer0 {} | fn {}",
                hex::encode(rt[i].encode()),
                l0[i].describe(),
                l1[i].describe()
            )
        })
        .collect();
    let _ = writeln!(
        s,
        "\nNon-physical indices with data: {}",
        if extra.is_empty() { "none" } else { "" }
    );
    for e in extra {
        let _ = writeln!(s, "{e}");
    }

    let _ = writeln!(s, "\n== Macros ==");
    if snap.macros.macros.is_empty() {
        let _ = writeln!(s, "none");
    }
    for m in &snap.macros.macros {
        let acts: Vec<String> = m
            .actions
            .iter()
            .map(|a| {
                format!(
                    "{}{:#04x}/k{}+{}ms",
                    if a.press() { "v" } else { "^" },
                    a.code,
                    a.kind(),
                    a.delay_ms
                )
            })
            .collect();
        let _ = writeln!(s, "#{:<3} @{:<4} {}", m.id, m.offset, acts.join(" "));
    }
    for e in &snap.macros.errors {
        let _ = writeln!(s, "table error: {e}");
    }

    let _ = writeln!(s, "\n== DKS slots ==");
    let slots: Vec<_> = dks::decode_block(&snap.dks.data)
        .into_iter()
        .enumerate()
        .filter(|(_, d)| !d.is_empty())
        .collect();
    if slots.is_empty() {
        let _ = writeln!(s, "none");
    }
    for (i, d) in slots {
        let _ = writeln!(
            s,
            "#{i:<2} make {:.1}/{:.1} mm break {:.1}/{:.1} mm actions {:02x?} phases {:02x?}",
            d.make[0] as f64 / 10.0,
            d.make[1] as f64 / 10.0,
            d.brk[0] as f64 / 10.0,
            d.brk[1] as f64 / 10.0,
            d.actions,
            d.phases
        );
    }
    s
}
