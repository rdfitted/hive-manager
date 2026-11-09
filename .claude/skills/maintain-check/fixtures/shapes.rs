// Shapes the checker must measure, plus text that must not confuse it.
pub trait Shape {
    fn declared_in_trait(&self) -> u32;
}

pub fn short_one(a: u32) -> u32 {
    a + 1
}

pub fn declared_only_in_impl<'a>(text: &'a str) -> &'a str {
    let brace = '}';
    let raw = r#"a raw string with } and "quotes""#;
    /* a block comment with } and /* nesting } */ still a comment */
    if text.contains(brace) || text.contains(raw) {
        return "found }";
    }
    text
}

pub fn long_one(start: u64) -> u64 {
    let v = start;
    let v = v + 0;
    let v = v + 1;
    let v = v + 2;
    let v = v + 3;
    let v = v + 4;
    let v = v + 5;
    let v = v + 6;
    let v = v + 7;
    let v = v + 8;
    let v = v + 9;
    let v = v + 10;
    let v = v + 11;
    let v = v + 12;
    let v = v + 13;
    let v = v + 14;
    let v = v + 15;
    let v = v + 16;
    let v = v + 17;
    let v = v + 18;
    let v = v + 19;
    let v = v + 20;
    let v = v + 21;
    let v = v + 22;
    let v = v + 23;
    let v = v + 24;
    let v = v + 25;
    let v = v + 26;
    let v = v + 27;
    let v = v + 28;
    let v = v + 29;
    let v = v + 30;
    let v = v + 31;
    let v = v + 32;
    let v = v + 33;
    let v = v + 34;
    let v = v + 35;
    let v = v + 36;
    let v = v + 37;
    let v = v + 38;
    let v = v + 39;
    let v = v + 40;
    let v = v + 41;
    let v = v + 42;
    let v = v + 43;
    let v = v + 44;
    let v = v + 45;
    let v = v + 46;
    let v = v + 47;
    let v = v + 48;
    let v = v + 49;
    let v = v + 50;
    let v = v + 51;
    let v = v + 52;
    let v = v + 53;
    let v = v + 54;
    let v = v + 55;
    let v = v + 56;
    let v = v + 57;
    let v = v + 58;
    let v = v + 59;
    let v = v + 60;
    let v = v + 61;
    let v = v + 62;
    let v = v + 63;
    let v = v + 64;
    let v = v + 65;
    let v = v + 66;
    let v = v + 67;
    let v = v + 68;
    let v = v + 69;
    let v = v + 70;
    let v = v + 71;
    let v = v + 72;
    let v = v + 73;
    let v = v + 74;
    let v = v + 75;
    let v = v + 76;
    let v = v + 77;
    let v = v + 78;
    let v = v + 79;
    let v = v + 80;
    let v = v + 81;
    let v = v + 82;
    let v = v + 83;
    let v = v + 84;
    let v = v + 85;
    let v = v + 86;
    let v = v + 87;
    let v = v + 88;
    let v = v + 89;
    let v = v + 90;
    let v = v + 91;
    let v = v + 92;
    let v = v + 93;
    let v = v + 94;
    let v = v + 95;
    let v = v + 96;
    let v = v + 97;
    let v = v + 98;
    let v = v + 99;
    let v = v + 100;
    let v = v + 101;
    let v = v + 102;
    let v = v + 103;
    let v = v + 104;
    let v = v + 105;
    let v = v + 106;
    let v = v + 107;
    let v = v + 108;
    let v = v + 109;
    v
}
