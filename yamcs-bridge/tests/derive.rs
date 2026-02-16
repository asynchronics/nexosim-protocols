#[cfg(feature = "derive")]
use nexosim_yamcs_bridge::YamcsValue;

#[cfg(feature = "derive")]
#[test]
fn derived_round_trip() {
    #[derive(Clone, Debug, PartialEq, Eq, YamcsValue)]
    struct Musician {
        name: String,
        born: u32,
    }

    #[derive(Clone, Debug, PartialEq, Eq, YamcsValue)]
    struct Band {
        name: String,
        musicians: Vec<Musician>,
    }

    let mike_kerr = Musician {
        name: "Mike Kerr".into(),
        born: 1990,
    };
    let ben_thatcher = Musician {
        name: "Ben Thatcher".into(),
        born: 1988,
    };
    let royal_blood = Band {
        name: "Royal Blood".into(),
        musicians: vec![mike_kerr, ben_thatcher],
    };

    // Check that the initial value is recovered after round-tripping.
    let encoded = royal_blood.clone().encode();
    let band: Band = YamcsValue::decode(encoded).expect("decoding error!");

    assert_eq!(band, royal_blood);
}
