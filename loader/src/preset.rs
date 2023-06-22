pub struct LocationPreset {
    pub title: &'static str,
    pub include: &'static [&'static str],
}

pub static PRESET: LocationPreset = LocationPreset {
    title: "New Nealand",
    include: &[
        "new+zealand",
        "auckland",
        "wellington",
        "christchurch",
        "hamilton",
        "tauranga",
        "napier-hastings",
        "dunedin",
        "palmerston+north",
        "nelson",
        "rotorua",
        "whangarei",
        "new+plymouth",
        "invercargill",
        "whanganui",
        "gisborne",
    ],
};
