/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
//! The world in braille: a land mask of the Natural Earth outline and points over it, for the geomap panel of a report and
//! the overview of the map component. Longitude and latitude are WGS84 degrees; the drawing spans 90°N to 60°S.

use crate::components::{Line, Span};
use crate::theme::Role;

/// Land at 2° cells, from 90°N (first row) to 60°S, 180°W to 180°E (first column the high bit): the country polygons of
/// `libs/naturalearth/countries-110m.json` (Natural Earth 110m, public domain), each cell set when its centre is on land,
/// with each ring made continuous across the antimeridian first (Russia, Fiji) and a ring around a pole left as it is.
const LAND: [&str; 75] = [
    "000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
    "0000000000007f800ffc0000000000000000000000000",
    "000000000017ff3fffffe000000000000040000000000",
    "00000000061df0ffffff0000f8000000003c000000000",
    "000000030027c3ffffff8000200000000006000000000",
    "0000000038afc007ffff00000000030007ffc001d8000",
    "0000000e88b7b003fffe000000000c003ffffec000000",
    "80180009fc33fc00fffa00000300041dffffffeffc801",
    "00fffbfe725d8f00fff800001ff000edfffffffffffdf",
    "e0ffffffffff87c1ff8000007ffebffdfffffffffffff",
    "31fffffffffd1f80fc01e000f9ebfffffffffffffffff",
    "00fffffffff8034078000003e7fffffffffffffffffff",
    "01fdffffffe01e0038000007e7ffffffffffffffff2f0",
    "007807ffffe01e4000000007e3ffffffffffffff82200",
    "001001fffff80fe000000182c7fffffffffffffe00f00",
    "0080007fffff9ff8000003820ffffffffffffffc00e00",
    "0000007fffff9ffc000006cfffffffffffffffffc0800",
    "0000003ffffffffc000000dfffffffffffffffff40000",
    "00000017ffffff460000007fffffffffffffffff40000",
    "0000000fffffffc10000007ffffffffffffffffe40000",
    "0000000fffffff600000007f7f97cffffffffffc00000",
    "0000000ffffffe00000007f19f03cffffffffff8c0000",
    "0000000ffffffc00000003c26f7fe7ffffffffe080000",
    "0000000ffffff000000007c02dffe7fffffff84080000",
    "00000007fffff0000000018740ffe7fffffffe6300000",
    "00000003fffff000000001fe022ffffffffffc4f00000",
    "00000001ffffc000000003ff000ffffffffffc1800000",
    "00000000ffff8000000007ffef7ffffffffffe0000000",
    "000000003fc04000000007ffffffdffffffffe0000000",
    "000000005f80400000001fffffefe7fffffffc0000000",
    "000000002f80000000001fffffe7f41ffffffc0000000",
    "000000000780c00000003ffffff7ff07fffff20000000",
    "000000000786100000007ffffffbfe07fcff000000000",
    "0000000003cc020000003ffffff9fe03f07e800000000",
    "0000000000fc000000003ffffff9f801e07f020000000",
    "00000000000f000000007ffffffde001c01f020000000",
    "000000000003000000003ffffffe8001c01f820000000",
    "0000000000010f0000003fffffff3000c013008000000",
    "000000000000aff000001fffffffe000a012000000000",
    "0000000000001ff800000fffffffe0002000008000000",
    "0000000000001fff00000687ffffc000002c180000000",
    "0000000000001fff80000001ffff80000014380000000",
    "0000000000003fff80000001ffff000000187a0000000",
    "0000000000003fffe0000003fffe0000000c781800000",
    "0000000000003ffffc000001fffc00000006762b80000",
    "0000000000007fffff000000fffc000000020101e0000",
    "0000000000003fffff800000fffc00000001c001f0800",
    "0000000000001fffff000000fffc000000000880d0200",
    "0000000000001ffffe0000007ffc00000000000000000",
    "0000000000000ffffe000000fffc20000000001c40000",
    "0000000000000ffffe000000fffc2000000000fc60040",
    "00000000000003fffc000000fff8e000000001fe60000",
    "00000000000001fffc000000fff0e000000001ffe0000",
    "00000000000001fffc0000007ff0c00000000ffff8080",
    "00000000000001fff00000007ff0c00000001ffff8000",
    "00000000000001ffc00000007fe0800000001ffffc000",
    "00000000000001ffc00000003fc0000000001ffffe000",
    "00000000000003ff800000003fc0000000001ffffe000",
    "00000000000003ff800000001f80000000000ffffe000",
    "00000000000003ff000000001f00000000000f07fc000",
    "00000000000003f8000000000000000000000801f8008",
    "00000000000007fc000000000000000000000000f8004",
    "00000000000007e000000000000000000000000000006",
    "00000000000007a00000000000000000000000003000c",
    "00000000000003c000000000000000000000000010018",
    "000000000000078000000000000000000000000000030",
    "000000000000078000000000000000000000000000000",
    "0000000000000f0000000000000000080000000000000",
    "0000000000000e0000000000000000000000000000000",
    "000000000000070000000000000000000000000000000",
    "000000000000030000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
    "000000000000000000000000000000000000000000000",
];

pub(crate) fn land(lon: f64, lat: f64) -> bool {
    #[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (column, row) = (
        ((lon + 180.0) / 2.0).floor() as usize,
        ((90.0 - lat) / 2.0).floor() as usize,
    );
    let Some(hex) = LAND.get(row) else {
        return false;
    };
    let column = column.min(179);
    hex.as_bytes()
        .get(column / 4)
        .and_then(|digit| (*digit as char).to_digit(16))
        .is_some_and(|digit| digit & (8 >> (column % 4)) != 0)
}

/// Each country of `libs/naturalearth/countries-110m.json` (Natural Earth 110m, public domain) with an ISO id: its ISO
/// 3166 alpha-3 (the Moqui geoId), alpha-2 and numeric codes and its extent `[west, south, east, north]` in degrees as the
/// browser's geomap gives it to Maps (`report.countryExtent` in WebrootReport.js): the bounding box of the outer ring
/// with the largest area, so an overseas territory does not widen it, each edge rounded to 1e-4. Each ring across the
/// antimeridian (Russia, Fiji) is unwrapped before its area is taken, so Russia's west is greater than its east and Fiji
/// is Viti Levu; one around a pole (Antarctica) keeps -180 to 180. The alpha codes are the ones the table of
/// `report.countryNumeric` gives that numeric code, "" where it gives none (South Sudan, and Sudan, which the table
/// knows by its former code 736). Generated ahead of time by loading WebrootReport.js and running its `topoCountries`
/// and `countryExtent` over the outline.
const COUNTRY_EXTENTS: [(&str, &str, &str, [f64; 4]); 174] = [
    ("AFG", "AF", "004", [60.5292, 29.3191, 75.1598, 38.486]),
    ("ALB", "AL", "008", [19.3052, 39.6251, 21.0188, 42.6886]),
    ("ATA", "AQ", "010", [-180.0, -85.609, 180.0, -63.2706]),
    ("DZA", "DZ", "012", [-8.6851, 19.0571, 12.0007, 37.1184]),
    ("AGO", "AO", "024", [11.6407, -17.9304, 24.0788, -5.8641]),
    ("AZE", "AZ", "031", [44.9734, 38.271, 50.3915, 41.8609]),
    ("ARG", "AR", "032", [-73.4137, -52.3503, -53.6279, -21.8317]),
    ("AUS", "AU", "036", [113.3381, -39.0366, 153.5685, -10.6676]),
    ("AUT", "AT", "040", [9.4807, 46.4325, 16.9796, 49.0391]),
    ("BHS", "BS", "044", [-78.407, 23.7099, -77.5358, 25.2095]),
    ("BGD", "BD", "050", [88.0839, 20.6701, 92.6739, 26.4468]),
    ("ARM", "AM", "051", [43.5838, 38.7416, 46.5071, 41.2482]),
    ("BEL", "BE", "056", [2.5146, 49.5299, 6.1579, 51.4747]),
    ("BTN", "BT", "064", [88.8147, 26.7193, 92.1051, 28.2968]),
    ("BOL", "BO", "068", [-69.5905, -22.8727, -57.498, -9.7621]),
    ("BIH", "BA", "070", [15.7484, 42.6497, 19.6004, 45.2342]),
    ("BWA", "BW", "072", [19.8956, -26.8282, 29.4321, -17.6613]),
    ("BRA", "BR", "076", [-73.9861, -33.7677, -34.7313, 5.2441]),
    ("BLZ", "BZ", "084", [-89.2287, 15.8869, -88.1055, 18.5002]),
    ("SLB", "SB", "090", [158.2126, -8.5384, 159.919, -7.3197]),
    ("BRN", "BN", "096", [114.2057, 4.0069, 115.4514, 5.4472]),
    ("BGR", "BG", "100", [22.3796, 41.2347, 28.5573, 44.2356]),
    ("MMR", "MM", "104", [92.3031, 9.9325, 101.1808, 28.3357]),
    ("BDI", "BI", "108", [29.0253, -4.4999, 30.7533, -2.3487]),
    ("BLR", "BY", "112", [23.2004, 51.319, 32.6937, 56.1698]),
    ("KHM", "KH", "116", [102.3472, 10.486, 107.6141, 14.5701]),
    ("CMR", "CM", "120", [8.4871, 1.727, 16.0112, 12.8589]),
    ("CAN", "CA", "124", [-140.9972, 41.6748, -55.6836, 71.9208]),
    ("CAF", "CF", "140", [14.4595, 2.2669, 27.3729, 11.1427]),
    ("LKA", "LK", "144", [79.6958, 5.9685, 81.7874, 9.8242]),
    ("TCD", "TD", "148", [13.5415, 7.4224, 23.888, 23.4104]),
    ("CHL", "CL", "152", [-75.6458, -53.8566, -66.9841, -17.58]),
    ("CHN", "CN", "156", [73.6765, 20.2825, 135.0248, 53.4583]),
    ("TWN", "TW", "158", [120.1062, 21.97, 121.953, 25.2959]),
    ("COL", "CO", "170", [-78.9902, -4.2985, -66.8761, 12.4375]),
    ("COG", "CG", "178", [11.0935, -5.0382, 18.452, 3.7276]),
    ("COD", "CD", "180", [12.1807, -13.2572, 31.1745, 5.256]),
    ("CRI", "CR", "188", [-85.9419, 8.2247, -82.547, 11.2172]),
    ("HRV", "HR", "191", [13.6567, 42.4804, 19.3916, 46.5036]),
    ("CUB", "CU", "192", [-84.9734, 19.856, -74.1769, 23.1886]),
    ("CYP", "CY", "196", [32.2581, 34.5711, 34.0041, 35.1736]),
    ("CZE", "CZ", "203", [12.2383, 48.555, 18.8516, 51.1175]),
    ("BEN", "BJ", "204", [0.7722, 6.1429, 3.7962, 12.2361]),
    ("DNK", "DK", "208", [8.0911, 54.831, 10.9135, 57.7304]),
    ("DOM", "DO", "214", [-71.9449, 17.5981, -68.3197, 19.8848]),
    ("ECU", "EC", "218", [-80.9666, -4.9586, -75.2354, 1.3817]),
    ("SLV", "SV", "222", [-90.0963, 13.1484, -87.7239, 14.4246]),
    ("GNQ", "GQ", "226", [9.3043, 1.0093, 11.2843, 2.2838]),
    ("ETH", "ET", "231", [32.9529, 3.4212, 47.7887, 14.9594]),
    ("ERI", "ER", "232", [36.3226, 12.4561, 43.0798, 17.9976]),
    ("EST", "EE", "233", [23.3408, 57.4748, 28.1325, 59.6108]),
    ("FLK", "FK", "238", [-61.1988, -52.2995, -57.75, -51.0995]),
    ("FJI", "FJ", "242", [177.2856, -18.2875, 178.7184, -17.3397]),
    ("FIN", "FI", "246", [20.6444, 59.8461, 31.5165, 70.1639]),
    ("FRA", "FR", "250", [-4.5918, 42.3433, 8.0983, 51.148]),
    ("ATF", "TF", "260", [68.7193, -49.7742, 70.5589, -48.6249]),
    ("DJI", "DJ", "262", [41.6614, 10.926, 43.3174, 12.6998]),
    ("GAB", "GA", "266", [8.7967, -3.9786, 14.4271, 2.3262]),
    ("GEO", "GE", "268", [39.955, 41.0637, 46.6367, 43.5535]),
    ("GMB", "GM", "270", [-16.8428, 13.1298, -13.8439, 13.8762]),
    ("PSE", "PS", "275", [34.9257, 31.3535, 35.545, 32.5332]),
    ("DEU", "DE", "276", [5.9887, 47.3025, 15.0176, 54.9833]),
    ("GHA", "GH", "288", [-3.2454, 4.711, 1.0602, 11.0987]),
    ("GRC", "GR", "300", [20.1512, 36.4092, 26.6025, 41.8271]),
    ("GRL", "GL", "304", [-73.2985, 60.0373, -12.2095, 83.6451]),
    ("GTM", "GT", "320", [-92.2275, 13.7357, -88.2243, 17.8198]),
    ("GIN", "GN", "324", [-15.1292, 7.309, -7.8319, 12.5864]),
    ("GUY", "GY", "328", [-61.4112, 1.2683, -56.5404, 8.3669]),
    ("HTI", "HT", "332", [-74.4577, 18.0314, -71.6245, 19.9152]),
    ("HND", "HN", "340", [-89.3547, 12.9842, -83.1482, 16.0054]),
    ("HUN", "HU", "348", [16.202, 45.7589, 22.7108, 48.6244]),
    ("ISL", "IS", "352", [-24.3272, 63.4969, -13.6099, 66.5266]),
    ("IND", "IN", "356", [68.1757, 7.9658, 97.4008, 35.4935]),
    ("IDN", "ID", "360", [108.9533, -4.1073, 118.9974, 4.3064]),
    ("IRN", "IR", "364", [44.1094, 25.0775, 63.3156, 39.7131]),
    ("IRQ", "IQ", "368", [38.7922, 29.099, 48.5663, 37.3858]),
    ("IRL", "IE", "372", [-9.9775, 51.6693, -6.0319, 55.1323]),
    ("ISR", "IL", "376", [34.2669, 29.5019, 35.8366, 33.278]),
    ("ITA", "IT", "380", [6.7483, 37.9088, 18.4808, 47.1146]),
    ("CIV", "CI", "384", [-8.6023, 4.3386, -2.5614, 10.5249]),
    ("JAM", "JM", "388", [-78.3386, 17.7014, -76.2002, 18.5239]),
    ("JPN", "JP", "392", [129.4087, 31.0303, 141.9152, 41.3786]),
    ("KAZ", "KZ", "398", [46.4675, 40.6626, 87.3603, 55.3845]),
    ("JOR", "JO", "400", [34.9221, 29.1972, 39.1954, 33.3795]),
    ("KEN", "KE", "404", [33.8925, -4.676, 41.8558, 5.5065]),
    ("PRK", "KP", "408", [124.2642, 37.6685, 130.7803, 42.9848]),
    ("KOR", "KR", "410", [126.1183, 34.39, 129.4699, 38.6129]),
    ("KWT", "KW", "414", [46.5683, 28.5253, 48.4151, 30.0587]),
    ("KGZ", "KG", "417", [69.4645, 39.2798, 80.261, 43.2979]),
    ("LAO", "LA", "418", [100.1152, 13.8812, 107.5637, 22.4642]),
    ("LBN", "LB", "422", [35.1274, 33.0884, 36.6106, 34.6456]),
    ("LSO", "LS", "426", [26.9985, -30.6449, 29.3241, -28.6477]),
    ("LVA", "LV", "428", [21.0548, 55.6147, 28.1757, 57.9707]),
    ("LBR", "LR", "430", [-11.4391, 4.3555, -7.5403, 8.5412]),
    ("LBY", "LY", "434", [9.3187, 19.5801, 25.1661, 33.1375]),
    ("LTU", "LT", "440", [21.0548, 53.9052, 26.5881, 56.3729]),
    ("LUX", "LU", "442", [5.6755, 49.4419, 6.2443, 50.1274]),
    ("MDG", "MG", "450", [43.2526, -25.6011, 50.4779, -12.0403]),
    ("MWI", "MW", "454", [32.6865, -16.8015, 35.7718, -9.2306]),
    ("MYS", "MY", "458", [109.6625, 0.7724, 119.181, 6.9282]),
    ("MLI", "ML", "466", [-12.1699, 10.0967, 4.2714, 24.9743]),
    ("MRT", "MR", "478", [-17.0624, 14.6175, -4.923, 27.3963]),
    ("MEX", "MX", "484", [-117.129, 14.5397, -86.8131, 32.7211]),
    ("MNG", "MN", "496", [87.7527, 41.5969, 119.7714, 52.0468]),
    ("MDA", "MD", "498", [26.6205, 45.4881, 30.0261, 48.467]),
    ("MNE", "ME", "499", [18.4484, 41.8779, 20.3384, 43.523]),
    ("MAR", "MA", "504", [-17.0192, 21.4199, -1.125, 35.7593]),
    ("MOZ", "MZ", "508", [30.1809, -26.7418, 40.7758, -10.3173]),
    ("OMN", "OM", "512", [52.0007, 16.6503, 59.8092, 24.9252]),
    ("NAM", "NA", "516", [11.7343, -29.0454, 25.0833, -16.9419]),
    ("NPL", "NP", "524", [80.0882, 26.3977, 88.1739, 30.4226]),
    ("NLD", "NL", "528", [3.3138, 50.8044, 7.0903, 53.5108]),
    ("NCL", "NC", "540", [164.0302, -22.4004, 167.1191, -20.1053]),
    ("VUT", "VU", "548", [166.6295, -15.7402, 167.2703, -14.6265]),
    ("NZL", "NZ", "554", [166.5107, -46.6413, 174.2471, -40.4939]),
    ("NIC", "NI", "558", [-87.6699, 10.7263, -83.1482, 15.017]),
    ("NER", "NE", "562", [0.297, 11.6606, 15.9032, 23.4713]),
    ("NGA", "NG", "566", [2.691, 4.2404, 14.5783, 13.866]),
    ("NOR", "NO", "578", [4.9914, 58.079, 31.2933, 71.1862]),
    ("PAK", "PK", "586", [60.8748, 23.6913, 77.8382, 37.1336]),
    ("PAN", "PA", "591", [-82.9646, 7.221, -77.2442, 9.6109]),
    ("PNG", "PG", "598", [141.0008, -10.6524, 150.8001, -2.6009]),
    ("PRY", "PY", "600", [-62.6856, -27.5492, -54.2939, -19.342]),
    ("PER", "PE", "604", [-81.4094, -18.3485, -68.6653, -0.057]),
    ("PHL", "PH", "608", [119.883, 12.5374, 124.1814, 18.5053]),
    ("POL", "PL", "616", [14.0743, 49.0272, 24.0284, 54.8513]),
    ("PRT", "PT", "620", [-9.5275, 36.8391, -6.3883, 42.2807]),
    ("GNB", "GW", "624", [-16.6772, 11.0411, -13.6999, 12.6288]),
    ("TLS", "TL", "626", [124.9698, -9.3931, 127.3351, -8.2727]),
    ("PRI", "PR", "630", [-67.2433, 17.9468, -65.5909, 18.5206]),
    ("QAT", "QA", "634", [50.7443, 24.5562, 51.6083, 26.1151]),
    ("ROU", "RO", "642", [20.2196, 43.6889, 29.6265, 48.2216]),
    ("RUS", "RU", "643", [27.2865, 41.1518, -169.8983, 77.6975]),
    ("RWA", "RW", "646", [29.0253, -2.9174, 30.8145, -1.1351]),
    ("SAU", "SA", "682", [34.6305, 16.3473, 55.6656, 32.1609]),
    ("SEN", "SN", "686", [-17.624, 12.3326, -11.4679, 16.5978]),
    ("SRB", "RS", "688", [18.83, 42.2451, 22.9844, 46.1719]),
    ("SLE", "SL", "694", [-13.2463, 6.786, -10.2295, 10.0476]),
    ("SVK", "SK", "703", [16.8788, 47.7578, 22.5596, 49.5722]),
    ("VNM", "VN", "704", [102.1708, 8.6005, 109.3349, 23.3528]),
    ("SVN", "SI", "705", [13.6963, 45.4525, 16.5656, 46.8523]),
    ("SOM", "SO", "706", [40.981, -1.6835, 51.1331, 12.0245]),
    ("ZAF", "ZA", "710", [16.346, -34.8187, 32.8305, -22.0907]),
    ("ZWE", "ZW", "716", [25.2633, -22.2718, 32.8485, -15.5083]),
    ("ESP", "ES", "724", [-9.3943, 35.9471, 3.0402, 43.7481]),
    ("", "", "728", [23.888, 3.5093, 35.2966, 12.2479]),
    ("", "", "729", [21.9368, 8.2298, 38.4106, 22.0005]),
    ("ESH", "EH", "732", [-17.0624, 21.0002, -8.6635, 27.657]),
    ("SUR", "SR", "740", [-58.0452, 1.8184, -53.9591, 6.0261]),
    ("SWZ", "SZ", "748", [30.6777, -27.2852, 32.0709, -25.6603]),
    ("SWE", "SE", "752", [11.0287, 55.3625, 23.9024, 69.1061]),
    ("CHE", "CH", "756", [6.0211, 45.7775, 10.4419, 47.8306]),
    ("SYR", "SY", "760", [35.6998, 32.3132, 42.349, 37.2301]),
    ("TJK", "TJ", "762", [67.4413, 36.7376, 74.9797, 40.9605]),
    ("THA", "TH", "764", [97.3756, 5.691, 105.5873, 20.4179]),
    ("TGO", "TG", "768", [-0.0486, 5.9296, 1.8666, 11.0191]),
    ("TTO", "TT", "780", [-61.9512, 10.0002, -60.8964, 10.8905]),
    ("ARE", "AE", "784", [51.5795, 22.4964, 56.3964, 26.0558]),
    ("TUN", "TN", "788", [7.5259, 30.3075, 11.4895, 37.3503]),
    ("TUR", "TR", "792", [26.1705, 35.8219, 44.7934, 42.0403]),
    ("TKM", "TM", "795", [52.5011, 35.2701, 66.5449, 42.7512]),
    ("UGA", "UG", "800", [29.5797, -1.4432, 35.0374, 4.2506]),
    ("UKR", "UA", "804", [22.0844, 45.2934, 40.081, 52.3345]),
    ("MKD", "MK", "807", [20.4644, 40.842, 22.952, 42.3196]),
    ("EGY", "EG", "818", [24.7016, 22.0005, 36.8662, 31.5854]),
    ("GBR", "GB", "826", [-6.1507, 49.9598, 1.683, 58.6342]),
    ("TZA", "TZ", "834", [29.3385, -11.7204, 40.315, -0.9506]),
    ("USA", "US", "840", [-124.6854, 25.0792, -66.9661, 49.3894]),
    ("BFA", "BF", "854", [-5.4703, 9.6109, 2.1762, 15.1168]),
    ("URY", "UY", "858", [-58.4268, -34.9525, -53.2103, -30.11]),
    ("UZB", "UZ", "860", [55.9284, 37.1455, 73.0537, 45.5863]),
    ("VEN", "VE", "862", [-73.3057, 0.725, -59.7588, 12.1616]),
    ("YEM", "YE", "887", [42.6046, 12.5864, 53.1095, 18.9995]),
    ("ZMB", "ZM", "894", [21.8864, -17.9609, 33.4857, -8.2388]),
];

/// A country's extent `[west, south, east, north]` by its ISO alpha-3, alpha-2 or numeric code, read as
/// `report.countryNumeric` reads one: trimmed, in any case, a numeric code with or without its leading zeros (728 and 729
/// too, which that table lacks). None for a code the outline has no country for.
pub(crate) fn country_extent(code: &str) -> Option<[f64; 4]> {
    let code = code.trim().to_ascii_uppercase();
    if code.is_empty() {
        return None;
    }
    let code = if code.len() <= 3 && code.bytes().all(|byte| byte.is_ascii_digit()) {
        format!("{code:0>3}")
    } else {
        code
    };
    COUNTRY_EXTENTS
        .iter()
        .find(|(alpha3, alpha2, numeric, _)| [*alpha3, *alpha2, *numeric].contains(&code.as_str()))
        .map(|(_, _, _, extent)| *extent)
}

/// The southern edge of the drawing, in degrees of latitude.
pub(crate) const SOUTH_EDGE: f64 = -60.0;

/// The world at `width` columns (at most 120) and a quarter as many rows (at least 6): land as dim dots, each of `points`
/// (longitude, latitude) a raised dot in the point colour and each of `marked` in the marked colour, a cell with a point
/// showing it over the land.
pub(crate) fn braille_map(points: &[(f64, f64)], marked: &[(f64, f64)], width: usize) -> Vec<Line> {
    let columns = width.clamp(1, 120);
    let rows = (columns / 4).max(6);
    let (dots_x, dots_y) = (columns * 2, rows * 4);
    let mut grid = vec![vec![0u8; columns]; rows];
    // 0 nothing, 1 a point, 2 a marked point
    let mut kinds = vec![vec![0u8; columns]; rows];
    let mut dot = |lon: f64, lat: f64, kind: u8| {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss
        )]
        let (x, y) = (
            (((lon + 180.0) / 360.0) * dots_x as f64)
                .floor()
                .clamp(0.0, (dots_x - 1) as f64) as usize,
            (((90.0 - lat) / 150.0) * dots_y as f64)
                .floor()
                .clamp(0.0, (dots_y - 1) as f64) as usize,
        );
        const BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
        grid[y / 4][x / 2] |= BITS[y % 4][x % 2];
        let cell = &mut kinds[y / 4][x / 2];
        *cell = (*cell).max(kind);
    };
    for (lon, lat) in points {
        dot(*lon, *lat, 1);
    }
    for (lon, lat) in marked {
        dot(*lon, *lat, 2);
    }
    // Land dots go under the points; a cell with a point shows it in the point colour.
    #[expect(clippy::cast_precision_loss)]
    for (r, line) in grid.iter_mut().enumerate() {
        for (c, cell) in line.iter_mut().enumerate() {
            if kinds[r][c] != 0 {
                continue;
            }
            let lon = -180.0 + (c as f64 + 0.5) * 360.0 / columns as f64;
            let lat = 90.0 - (r as f64 + 0.5) * 150.0 / rows as f64;
            if land(lon, lat) {
                *cell = 0x24;
            }
        }
    }
    grid.iter()
        .zip(&kinds)
        .map(|(line, kinds)| {
            line.iter()
                .zip(kinds)
                .map(|(bits, kind)| {
                    let glyph = char::from_u32(0x2800 + u32::from(*bits)).unwrap_or(' ');
                    let role = match kind {
                        2 => Role::Primary,
                        1 => Role::Negative,
                        _ => Role::Muted,
                    };
                    Span::new(glyph.to_string(), role)
                })
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn land_is_the_outline_and_points_go_over_it() {
        assert!(land(2.35, 48.85), "Paris is on land");
        assert!(!land(-30.0, 30.0), "the mid Atlantic is not");
        // the rings across the antimeridian are not filled across the world
        assert!(land(100.0, 69.0), "the Siberian coast is land");
        assert!(!land(0.0, 69.0), "the Norwegian Sea is not");
        assert!(land(-30.0, 71.0), "Greenland is land at 71°N");
        assert!(!land(-5.0, 71.0), "the Greenland Sea is not");
        assert!(
            land(178.0, 67.0),
            "Chukotka, past the antimeridian of its ring, is land"
        );
        let lines = braille_map(&[(2.35, 48.85)], &[(151.21, -33.87)], 60);
        assert_eq!(lines.len(), 15);
        assert!(lines.iter().all(|line| line.len() == 60));
        let roles = |role| lines.iter().flatten().filter(|span| span.role == role).count();
        assert_eq!(roles(Role::Negative), 1, "the point");
        assert_eq!(roles(Role::Primary), 1, "the marked point");
        assert!(roles(Role::Muted) > 0, "the land and sea");
        assert!(lines.iter().flatten().any(|span| span.text == "\u{2824}"), "land dots");
    }

    #[test]
    fn a_country_has_one_extent_by_any_of_its_codes() {
        // mainland France, not French Guiana (the browser's page test has the same bounds)
        let france = country_extent("FRA").unwrap();
        assert!(
            france[0] > -6.0 && france[2] < 10.0 && france[1] > 41.0 && france[3] < 52.0,
            "{france:?}"
        );
        assert_eq!(france, [-4.5918, 42.3433, 8.0983, 51.148]);
        for code in ["fr", " FR ", "250"] {
            assert_eq!(country_extent(code), Some(france), "{code}");
        }
        assert_eq!(
            country_extent("40"),
            country_extent("AUT"),
            "a numeric code without its leading zero"
        );
        assert_eq!(country_extent("040"), country_extent("at"));
        // a country across the antimeridian has its own extent, west greater than east; one around a pole the world's.
        // Fiji's rings are unwrapped before their areas are compared: Viti Levu, not Vanua Levu across the antimeridian
        assert_eq!(country_extent("FJI"), Some([177.2856, -18.2875, 178.7184, -17.3397]));
        let russia = country_extent("RU").unwrap();
        assert!(russia[0] > 0.0 && russia[2] < 0.0, "{russia:?}");
        assert_eq!(
            country_extent("ATA").map(|extent| (extent[0], extent[2])),
            Some((-180.0, 180.0))
        );
        for unknown in ["", "XYZ", "999", "FXX", "0250"] {
            assert_eq!(country_extent(unknown), None, "{unknown}");
        }
    }
}
