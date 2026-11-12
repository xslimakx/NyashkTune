//! Parameter texts: typing back what the plugin shows must give the same value (click a value
//! box, press Enter: nothing may change), and no value may show as "-0".
use nih_plug::prelude::*;

#[test]
fn displayed_values_parse_back() {
    let p = nyashktune::NyashkTuneParams::default();
    let mut bad: Vec<String> = Vec::new();
    for (id, ptr, _) in p.param_map() {
        let mut reported = false;
        for i in 0..=400 {
            let n = i as f32 / 400.0;
            let (shown, bare) = unsafe { (ptr.normalized_value_to_string(n, true), ptr.normalized_value_to_string(n, false)) };
            // what a value box starts editing with: the bare number, or the full text when the
            // number alone would mean something else
            let first = shown.trim().split(' ').next().unwrap_or("").to_string();
            let same = unsafe { ptr.string_to_normalized_value(&first).map(|m| ptr.normalized_value_to_string(m, true)) }.as_deref() == Some(shown.as_str());
            let prefill = if same { first } else { shown.trim().to_string() };
            for txt in [shown.clone(), bare.clone(), prefill] {
                let back = unsafe { ptr.string_to_normalized_value(&txt).map(|m| ptr.normalized_value_to_string(m, true)) };
                if back.as_deref() != Some(shown.as_str()) && !reported {
                    bad.push(format!("{id}: typed '{txt}' -> {:?}, shown '{shown}'", back));
                    reported = true;
                }
            }
            let num = shown.trim().split(' ').next().unwrap_or("");
            if (num == "-0" || num.starts_with("-0.") && num[3..].chars().all(|c| c == '0')) && !reported {
                bad.push(format!("{id}: shows negative zero '{shown}'"));
                reported = true;
            }
        }
    }
    for b in &bad {
        println!("BAD: {b}");
    }
    assert!(bad.is_empty(), "{} parameters have text problems", bad.len());
}
