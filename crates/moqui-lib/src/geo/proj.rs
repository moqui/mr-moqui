/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! The projections of the map registry (`moqui.maps.MapProjection`, plans/Maps.md §4.1, P4c): a CRS's PROJ.4 definition,
//! the same text the browser registers with proj4js, read here for the few methods a registered row may use, so a user's
//! overlay in one of them is converted to WGS84 longitude and latitude on the server instead of refused.
//!
//! - Methods: geographic (`longlat`), Mercator (`merc`, ellipsoidal or the Web Mercator sphere), transverse Mercator
//!   (`tmerc`, `utm`: Krüger's series to the sixth order, as Karney 2011 gives them, millimetres within a few thousand
//!   kilometres of the central meridian), Lambert conformal conic (`lcc`, one or two standard parallels), Lambert azimuthal
//!   equal-area (`laea`, oblique and equatorial aspects), Albers equal-area (`aea`) and polar stereographic (`stere` at
//!   a pole, with or without `lat_ts`), each ellipsoidal as the EPSG Guidance Note 7-2 and Snyder (1987) give them.
//! - Datums: a datum whose shift to WGS84 is zero (`+datum=WGS84`, `+datum=NAD83`, `+towgs84=0,0,0...`, ETRS89 and RGF93
//!   written so) is taken as WGS84. NAD83 (2011) and ETRS89 differ from WGS84 (G2139) by one to two metres today (the
//!   plates' drift since 1989 for ETRS89), which a map at the scales an overlay is drawn does not show; the transform
//!   recorded says so. A three- or seven-parameter `+towgs84` (and `+datum=OSGB36`, Airy 1830 with the parameters
//!   PROJ gives it) is a Helmert transformation of the position vector convention through geocentric coordinates, the
//!   EPSG method 9606. For OSGB36 that is the Ordnance Survey's approximation, good to about five metres; the OSTN15
//!   grid, which is exact to centimetres, is not used. No grid shift (`+nadgrids`, `+geoidgrids`) is read: a definition
//!   that needs one (NAD27) is refused, never drawn wrong.
//! - Anything else a definition names (another method, a prime meridian other than Greenwich, an unknown parameter) is
//!   refused by name, so a registered row is either read as written or not used on the server.
//!
//! Coordinates are in the definition's units (`+units=m`, `ft`, `us-ft`, or `+to_meter`), degrees for a geographic CRS,
//! easting first. The crate has no PROJ: `proj4rs` (MIT or Apache-2.0) would read every PROJ.4 definition, but brings a
//! port of proj4js for a handful of methods that these few hundred lines cover with their own tests
//! (`proj_tests.rs`: the EPSG guidance examples).

use std::collections::BTreeMap;
use std::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

/// An ellipsoid: its semi-major axis in metres and its flattening.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ellipsoid {
    pub a: f64,
    pub f: f64,
    pub name: &'static str,
}

impl Ellipsoid {
    pub const WGS84: Self = Self {
        a: 6_378_137.0,
        f: 1.0 / 298.257_223_563,
        name: "WGS 84",
    };
    pub const GRS80: Self = Self {
        a: 6_378_137.0,
        f: 1.0 / 298.257_222_101,
        name: "GRS 1980",
    };

    fn e2(self) -> f64 {
        self.f * (2.0 - self.f)
    }

    fn e(self) -> f64 {
        self.e2().sqrt()
    }

    fn by_name(name: &str) -> Option<Self> {
        let (a, f, name) = match name.to_ascii_lowercase().as_str() {
            "wgs84" => return Some(Self::WGS84),
            "grs80" => return Some(Self::GRS80),
            "airy" => (6_377_563.396, 1.0 - 6_356_256.909 / 6_377_563.396, "Airy 1830"),
            "intl" => (6_378_388.0, 1.0 / 297.0, "International 1924"),
            "clrk66" => (6_378_206.4, 1.0 - 6_356_583.8 / 6_378_206.4, "Clarke 1866"),
            "bessel" => (6_377_397.155, 1.0 / 299.152_812_8, "Bessel 1841"),
            "krass" => (6_378_245.0, 1.0 / 298.3, "Krassowsky 1940"),
            "sphere" => (6_370_997.0, 0.0, "a sphere"),
            _ => return None,
        };
        Some(Self { a, f, name })
    }
}

/// A Helmert transformation to WGS84, the position vector convention (EPSG method 9606, PROJ's `+towgs84`): metres,
/// rotations in radians, the scale as a factor less one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Helmert {
    pub tx: f64,
    pub ty: f64,
    pub tz: f64,
    pub rx: f64,
    pub ry: f64,
    pub rz: f64,
    pub ds: f64,
}

impl Helmert {
    /// From `+towgs84`'s three or seven numbers: metres, then arc-seconds and parts per million.
    fn parse(text: &str) -> Result<Self, String> {
        let numbers = text
            .split(',')
            .map(|part| part.trim().parse::<f64>().ok().filter(|value| value.is_finite()))
            .collect::<Option<Vec<f64>>>()
            .ok_or_else(|| format!("+towgs84={text} is not a list of numbers"))?;
        let arc = |seconds: f64| (seconds / 3600.0).to_radians();
        match numbers.as_slice() {
            [tx, ty, tz] => Ok(Self {
                tx: *tx,
                ty: *ty,
                tz: *tz,
                rx: 0.0,
                ry: 0.0,
                rz: 0.0,
                ds: 0.0,
            }),
            [tx, ty, tz, rx, ry, rz, ds] => Ok(Self {
                tx: *tx,
                ty: *ty,
                tz: *tz,
                rx: arc(*rx),
                ry: arc(*ry),
                rz: arc(*rz),
                ds: ds * 1e-6,
            }),
            _ => Err(format!("+towgs84={text} has neither three nor seven numbers")),
        }
    }

    fn is_zero(&self) -> bool {
        [self.tx, self.ty, self.tz, self.rx, self.ry, self.rz, self.ds]
            .iter()
            .all(|value| *value == 0.0)
    }

    /// Geocentric coordinates of the source datum to WGS84's.
    #[must_use]
    pub fn forward(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let m = 1.0 + self.ds;
        [
            self.tx + m * (x - self.rz * y + self.ry * z),
            self.ty + m * (self.rz * x + y - self.rx * z),
            self.tz + m * (-self.ry * x + self.rx * y + z),
        ]
    }

    /// WGS84's geocentric coordinates to the source datum's: the exact inverse of [`Helmert::forward`].
    #[must_use]
    pub fn inverse(&self, [x, y, z]: [f64; 3]) -> [f64; 3] {
        let m = 1.0 + self.ds;
        let (u, v, w) = ((x - self.tx) / m, (y - self.ty) / m, (z - self.tz) / m);
        // R = I + S, S skew; R⁻¹ by solving the 3x3 system exactly (Cramer's rule)
        let r = [
            [1.0, -self.rz, self.ry],
            [self.rz, 1.0, -self.rx],
            [-self.ry, self.rx, 1.0],
        ];
        let det = r[0][0] * (r[1][1] * r[2][2] - r[1][2] * r[2][1]) - r[0][1] * (r[1][0] * r[2][2] - r[1][2] * r[2][0])
            + r[0][2] * (r[1][0] * r[2][1] - r[1][1] * r[2][0]);
        let solve = |column: usize| {
            let mut m = r;
            for (row, value) in [u, v, w].iter().enumerate() {
                m[row][column] = *value;
            }
            (m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
                + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]))
                / det
        };
        [solve(0), solve(1), solve(2)]
    }
}

/// Geodetic latitude and longitude (radians) and height on an ellipsoid to geocentric X, Y, Z.
#[must_use]
pub fn geodetic_to_geocentric(ellipsoid: Ellipsoid, lon: f64, lat: f64, height: f64) -> [f64; 3] {
    let e2 = ellipsoid.e2();
    let nu = ellipsoid.a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
    [
        (nu + height) * lat.cos() * lon.cos(),
        (nu + height) * lat.cos() * lon.sin(),
        ((1.0 - e2) * nu + height) * lat.sin(),
    ]
}

/// Geocentric X, Y, Z to geodetic longitude and latitude (radians) on an ellipsoid (iterated to well below a millimetre).
#[must_use]
pub fn geocentric_to_geodetic(ellipsoid: Ellipsoid, [x, y, z]: [f64; 3]) -> (f64, f64) {
    let e2 = ellipsoid.e2();
    let p = x.hypot(y);
    let lon = y.atan2(x);
    let mut lat = z.atan2(p * (1.0 - e2));
    for _ in 0..10 {
        let nu = ellipsoid.a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        let next = (z + e2 * nu * lat.sin()).atan2(p);
        if (next - lat).abs() < 1e-14 {
            lat = next;
            break;
        }
        lat = next;
    }
    (lon, lat)
}

/// A projection method with its parameters (angles in radians, false easting and northing in metres).
#[derive(Clone, Debug, PartialEq)]
pub enum Method {
    LongLat,
    Mercator {
        lon0: f64,
        k0: f64,
        x0: f64,
        y0: f64,
    },
    TransverseMercator {
        lat0: f64,
        lon0: f64,
        k0: f64,
        x0: f64,
        y0: f64,
    },
    LambertConformal {
        lat0: f64,
        lon0: f64,
        lat1: f64,
        lat2: f64,
        k0: f64,
        x0: f64,
        y0: f64,
    },
    LambertAzimuthal {
        lat0: f64,
        lon0: f64,
        x0: f64,
        y0: f64,
    },
    Albers {
        lat0: f64,
        lon0: f64,
        lat1: f64,
        lat2: f64,
        x0: f64,
        y0: f64,
    },
    PolarStereographic {
        north: bool,
        lat_ts: Option<f64>,
        lon0: f64,
        k0: f64,
        x0: f64,
        y0: f64,
    },
}

impl Method {
    fn name(&self) -> &'static str {
        match self {
            Self::LongLat => "geographic",
            Self::Mercator { .. } => "Mercator",
            Self::TransverseMercator { .. } => "transverse Mercator",
            Self::LambertConformal { .. } => "Lambert conformal conic",
            Self::LambertAzimuthal { .. } => "Lambert azimuthal equal-area",
            Self::Albers { .. } => "Albers equal-area",
            Self::PolarStereographic { .. } => "polar stereographic",
        }
    }
}

/// A CRS of the registry: its method, ellipsoid, shift to WGS84 and units.
#[derive(Clone, Debug, PartialEq)]
pub struct Projection {
    pub method: Method,
    pub ellipsoid: Ellipsoid,
    /// None: the datum is taken as WGS84 (a zero shift)
    pub to_wgs84: Option<Helmert>,
    /// metres in one unit of the coordinates (1 for metres; unused for a geographic CRS, in degrees)
    pub to_meter: f64,
    /// the datum as the definition named it, for the provenance (`NAD83`, `OSGB36`), empty when none was named
    pub datum: String,
    /// a `+towgs84` of zeros was given: the datum is taken as WGS84 because the definition says so
    pub zero_shift_given: bool,
}

/// The parameters PROJ.4 definitions carry that change nothing here.
const IGNORED: [&str; 6] = ["no_defs", "wktext", "type", "title", "axis", "no_uoff"];

impl Projection {
    /// A PROJ.4 definition (`+proj=utm +zone=33 +datum=WGS84 +units=m +no_defs`), the registry's text.
    ///
    /// # Errors
    /// The parameter refused or missing, in words: a method, datum, ellipsoid, unit or parameter not read here, a grid.
    pub fn parse_proj4(definition: &str) -> Result<Self, String> {
        let mut parameters: BTreeMap<String, String> = BTreeMap::new();
        for token in definition.split_whitespace() {
            let token = token
                .strip_prefix('+')
                .ok_or_else(|| format!("{token} is not a +parameter"))?;
            let (key, value) = token.split_once('=').unwrap_or((token, ""));
            let key = key.to_ascii_lowercase();
            if IGNORED.contains(&key.as_str()) {
                continue;
            }
            if parameters.insert(key.clone(), value.to_string()).is_some() {
                return Err(format!("+{key} is given twice"));
            }
        }
        let mut take = |key: &str| parameters.remove(key);
        let proj = take("proj").ok_or("the definition names no +proj")?;
        // a latitude parameter within ±90°, a longitude within ±360°
        let angle = |value: Option<String>, name: &str| -> Result<f64, String> {
            let limit = if name.starts_with("lat") { 90.0 } else { 360.0 };
            value.map_or(Ok(0.0), |text| {
                text.parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite() && value.abs() <= limit)
                    .map(f64::to_radians)
                    .ok_or_else(|| format!("+{name}={text} is not an angle in degrees (within {limit})"))
            })
        };
        let number = |value: Option<String>, name: &str, default: f64| -> Result<f64, String> {
            value.map_or(Ok(default), |text| {
                text.parse::<f64>()
                    .ok()
                    .filter(|value| value.is_finite())
                    .ok_or_else(|| format!("+{name}={text} is not a number"))
            })
        };
        // the datum and the ellipsoid
        let datum_name = take("datum").unwrap_or_default();
        let towgs84 = take("towgs84");
        let grids = take("nadgrids");
        let (mut ellipsoid, mut shift) = match datum_name.to_ascii_lowercase().as_str() {
            "" => (Ellipsoid::WGS84, None),
            "wgs84" => (Ellipsoid::WGS84, None),
            "nad83" => (Ellipsoid::GRS80, None),
            "osgb36" => (
                Ellipsoid::by_name("airy").ok_or("airy")?,
                Some(Helmert::parse("446.448,-125.157,542.06,0.1502,0.247,0.8421,-20.4894")?),
            ),
            _ => {
                return Err(format!(
                    "+datum={datum_name} is not read here (it needs a grid shift or is not known): give +ellps and +towgs84"
                ))
            }
        };
        if let Some(name) = take("ellps") {
            ellipsoid =
                Ellipsoid::by_name(&name).ok_or_else(|| format!("+ellps={name} is not an ellipsoid read here"))?;
        }
        let a = take("a");
        let b = take("b");
        let rf = take("rf");
        let f = take("f");
        let r = take("r");
        if let Some(r) = r {
            ellipsoid = Ellipsoid {
                a: number(Some(r), "R", 0.0)?,
                f: 0.0,
                name: "a sphere",
            };
        } else if let Some(a) = a {
            let a = number(Some(a), "a", 0.0)?;
            let flattening = if let Some(b) = b {
                1.0 - number(Some(b), "b", 0.0)? / a
            } else if let Some(rf) = rf {
                1.0 / number(Some(rf), "rf", 0.0)?
            } else if let Some(f) = f {
                number(Some(f), "f", 0.0)?
            } else {
                ellipsoid.f
            };
            ellipsoid = Ellipsoid {
                a,
                f: flattening,
                name: if flattening == 0.0 {
                    "a sphere"
                } else {
                    "a given ellipsoid"
                },
            };
        }
        if !(ellipsoid.a > 6_000_000.0 && ellipsoid.a < 7_000_000.0 && (0.0..0.01).contains(&ellipsoid.f)) {
            return Err("its ellipsoid is not the Earth's".to_string());
        }
        if let Some(grids) = &grids {
            // "@null" is Web Mercator's way of saying no datum shift; any real grid is refused
            if grids != "@null" {
                return Err(format!("+nadgrids={grids}: a grid shift is not read here"));
            }
        }
        // an ellipsoid other than WGS 84's or GRS 1980's names a datum of its own: with no datum and no +towgs84 the
        // definition does not say how to reach WGS84 (taking it as WGS84 would be 100 to 200 m off), so it is refused;
        // the Web Mercator sphere (+nadgrids=@null) is WGS84 by definition
        let close = |a: Ellipsoid, b: Ellipsoid| (a.a - b.a).abs() < 1e-3 && (a.f - b.f).abs() < 1e-12;
        let web_sphere =
            proj == "merc" && ellipsoid.f == 0.0 && (ellipsoid.a - 6_378_137.0).abs() < 1e-3 && grids.is_some();
        if datum_name.is_empty()
            && towgs84.is_none()
            && !close(ellipsoid, Ellipsoid::WGS84)
            && !close(ellipsoid, Ellipsoid::GRS80)
            && !web_sphere
        {
            return Err(format!(
                "its ellipsoid ({}) is not WGS 84's and it gives no datum shift: add +towgs84 (+towgs84=0,0,0 if it is to be taken as WGS84)",
                ellipsoid.name
            ));
        }
        let mut zero_shift_given = false;
        if let Some(text) = towgs84 {
            let helmert = Helmert::parse(&text)?;
            zero_shift_given = helmert.is_zero();
            shift = (!helmert.is_zero()).then_some(helmert);
        }
        for refused in ["geoidgrids", "pm", "lon_wrap", "over", "approx"] {
            if let Some(value) = take(refused) {
                if !(refused == "pm" && value.eq_ignore_ascii_case("greenwich")) {
                    return Err(format!("+{refused} is not read here"));
                }
            }
        }
        let to_meter = match (take("units"), take("to_meter")) {
            (_, Some(text)) => number(Some(text), "to_meter", 1.0)?,
            (Some(units), None) => match units.as_str() {
                "m" => 1.0,
                "km" => 1000.0,
                "ft" => 0.3048,
                "us-ft" => 1200.0 / 3937.0,
                other => return Err(format!("+units={other} is not read here")),
            },
            (None, None) => 1.0,
        };
        let lat0 = angle(take("lat_0"), "lat_0")?;
        let lon0 = angle(take("lon_0"), "lon_0")?;
        let lat1 = take("lat_1");
        let lat2 = take("lat_2");
        let lat_ts = take("lat_ts");
        let k_given = take("k_0").or_else(|| take("k"));
        let has_k = k_given.is_some();
        let k0 = number(k_given, "k", 1.0)?;
        let x0 = number(take("x_0"), "x_0", 0.0)?;
        let y0 = number(take("y_0"), "y_0", 0.0)?;
        let zone = take("zone");
        let south = take("south").is_some();
        // a parameter its method does not use is refused, not passed over: a definition is read as written or not at all
        let unused = |name: &str| Err(format!("+{name} does not apply to +proj={proj}"));
        if zone.is_some() && proj != "utm" {
            return unused("zone");
        }
        if south && proj != "utm" && proj != "ups" {
            return unused("south");
        }
        if lat_ts.is_some() && !matches!(proj.as_str(), "merc" | "stere") {
            return unused("lat_ts");
        }
        // PROJ writes +k=1 beside +lat_ts (EPSG:3413's); another scale with a latitude of true scale is two answers
        if lat_ts.is_some() && has_k && (k0 - 1.0).abs() > 1e-12 {
            return Err("+k and +lat_ts both set the scale: give one".to_string());
        }
        if has_k && !matches!(proj.as_str(), "tmerc" | "etmerc" | "merc" | "lcc" | "stere") {
            return unused("k");
        }
        let method = match proj.as_str() {
            "longlat" | "latlong" | "lonlat" | "latlon" => Method::LongLat,
            "merc" => Method::Mercator {
                lon0,
                k0: match &lat_ts {
                    Some(text) => {
                        let phi = angle(Some(text.clone()), "lat_ts")?;
                        m_of(ellipsoid, phi)
                    }
                    None => k0,
                },
                x0,
                y0,
            },
            "tmerc" | "etmerc" => Method::TransverseMercator { lat0, lon0, k0, x0, y0 },
            "utm" => {
                let zone: u8 = zone
                    .as_deref()
                    .and_then(|zone| zone.parse().ok())
                    .filter(|zone| (1..=60).contains(zone))
                    .ok_or("+proj=utm needs a +zone from 1 to 60")?;
                Method::TransverseMercator {
                    lat0: 0.0,
                    lon0: (f64::from(zone) * 6.0 - 183.0).to_radians(),
                    k0: 0.9996,
                    x0: 500_000.0,
                    y0: if south { 10_000_000.0 } else { 0.0 },
                }
            }
            "lcc" => {
                let lat1 = angle(lat1.clone(), "lat_1")?;
                let lat2 = match &lat2 {
                    Some(_) => angle(lat2.clone(), "lat_2")?,
                    None => lat1,
                };
                if lat1.abs() >= FRAC_PI_2 || lat2.abs() >= FRAC_PI_2 || (lat1 + lat2).abs() < 1e-10 {
                    return Err("+proj=lcc needs standard parallels off the poles, not opposite".to_string());
                }
                Method::LambertConformal {
                    lat0,
                    lon0,
                    lat1,
                    lat2,
                    k0,
                    x0,
                    y0,
                }
            }
            "laea" => {
                if lat0.abs() > FRAC_PI_2 - 1e-9 {
                    return Err("+proj=laea at a pole (its polar aspect) is not read here".to_string());
                }
                Method::LambertAzimuthal { lat0, lon0, x0, y0 }
            }
            "aea" => {
                let lat1 = angle(lat1.clone(), "lat_1")?;
                let lat2 = match &lat2 {
                    Some(_) => angle(lat2.clone(), "lat_2")?,
                    None => lat1,
                };
                if (lat1 + lat2).abs() < 1e-10 {
                    return Err("+proj=aea needs standard parallels that are not opposite".to_string());
                }
                Method::Albers {
                    lat0,
                    lon0,
                    lat1,
                    lat2,
                    x0,
                    y0,
                }
            }
            "stere" | "ups" => {
                let north = if proj == "ups" {
                    !south
                } else if (lat0 - FRAC_PI_2).abs() < 1e-9 {
                    true
                } else if (lat0 + FRAC_PI_2).abs() < 1e-9 {
                    false
                } else {
                    return Err("+proj=stere is read only at a pole (+lat_0=90 or -90)".to_string());
                };
                Method::PolarStereographic {
                    north,
                    lat_ts: match lat_ts {
                        Some(text) => Some(angle(Some(text), "lat_ts")?),
                        None => None,
                    },
                    lon0,
                    k0: if proj == "ups" { 0.994 } else { k0 },
                    x0: if proj == "ups" { 2_000_000.0 } else { x0 },
                    y0: if proj == "ups" { 2_000_000.0 } else { y0 },
                }
            }
            other => return Err(format!("+proj={other} is not a method read here")),
        };
        if let Some(extra) = parameters.keys().next() {
            return Err(format!("+{extra} is not read here"));
        }
        if matches!(method, Method::LongLat) && to_meter != 1.0 {
            return Err("a geographic CRS is in degrees: +units does not apply".to_string());
        }
        Ok(Self {
            method,
            ellipsoid,
            to_wgs84: shift,
            to_meter,
            datum: datum_name,
            zero_shift_given,
        })
    }

    /// Whether the CRS is geographic (degrees), not projected.
    #[must_use]
    pub fn is_geographic(&self) -> bool {
        matches!(self.method, Method::LongLat)
    }

    /// What converting from it to WGS84 does, in words, for an overlay's provenance.
    #[must_use]
    pub fn describe(&self) -> String {
        let mut out = if self.is_geographic() {
            format!("geographic coordinates on {}", self.ellipsoid.name)
        } else {
            format!("inverse {} on {}", self.method.name(), self.ellipsoid.name)
        };
        match &self.to_wgs84 {
            Some(helmert) if helmert.rx == 0.0 && helmert.ry == 0.0 && helmert.rz == 0.0 && helmert.ds == 0.0 => {
                out.push_str(", then a three-parameter shift to WGS84 (a few metres)");
            }
            Some(_) if self.datum.eq_ignore_ascii_case("osgb36") || self.ellipsoid.name == "Airy 1830" => out.push_str(
                ", then the seven-parameter Helmert shift from OSGB36 to WGS84 (about 5 m; the OSTN15 grid is not used)",
            ),
            Some(_) => out.push_str(", then a seven-parameter Helmert shift to WGS84 (a few metres)"),
            None if self.datum.eq_ignore_ascii_case("nad83") || self.ellipsoid == Ellipsoid::GRS80 => {
                out.push_str(", its datum taken as WGS84 (within about 2 m)");
            }
            None if self.zero_shift_given && self.ellipsoid != Ellipsoid::WGS84 => {
                out.push_str(", its datum taken as WGS84 as its +towgs84 of zeros says (no shift between the datums applied)");
            }
            None => out.push_str(", WGS84"),
        }
        out
    }

    /// Coordinates of this CRS (easting, northing in its units, or longitude, latitude in degrees) to WGS84 longitude and
    /// latitude in degrees; None for a point outside the projection's domain.
    #[must_use]
    pub fn to_wgs84(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        if !x.is_finite() || !y.is_finite() {
            return None;
        }
        let (lon, lat) = if self.is_geographic() {
            (x.to_radians(), y.to_radians())
        } else {
            self.inverse(x * self.to_meter, y * self.to_meter)?
        };
        let (lon, lat) = match &self.to_wgs84 {
            Some(helmert) => {
                let wgs = helmert.forward(geodetic_to_geocentric(self.ellipsoid, lon, lat, 0.0));
                geocentric_to_geodetic(Ellipsoid::WGS84, wgs)
            }
            None => (lon, lat),
        };
        let (lon, lat) = (normalize_lon(lon).to_degrees(), lat.to_degrees());
        (lon.is_finite() && lat.is_finite() && lat.abs() <= 90.0 + 1e-9).then_some((lon, lat.clamp(-90.0, 90.0)))
    }

    /// WGS84 longitude and latitude in degrees to this CRS's coordinates; None outside its domain.
    #[must_use]
    pub fn from_wgs84(&self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        if !lon.is_finite() || !lat.is_finite() || lat.abs() > 90.0 {
            return None;
        }
        let (lon, lat) = (lon.to_radians(), lat.to_radians());
        let (lon, lat) = match &self.to_wgs84 {
            Some(helmert) => {
                let local = helmert.inverse(geodetic_to_geocentric(Ellipsoid::WGS84, lon, lat, 0.0));
                geocentric_to_geodetic(self.ellipsoid, local)
            }
            None => (lon, lat),
        };
        if self.is_geographic() {
            return Some((normalize_lon(lon).to_degrees(), lat.to_degrees()));
        }
        let (x, y) = self.forward(lon, lat)?;
        Some((x / self.to_meter, y / self.to_meter))
    }

    /// Geodetic (radians, this ellipsoid) to projected metres.
    fn forward(&self, lon: f64, lat: f64) -> Option<(f64, f64)> {
        let el = self.ellipsoid;
        let (a, e) = (el.a, el.e());
        match self.method {
            Method::LongLat => Some((lon, lat)),
            Method::Mercator { lon0, k0, x0, y0 } => {
                if lat.abs() >= FRAC_PI_2 {
                    return None;
                }
                let t = t_of(e, lat);
                Some((x0 + a * k0 * normalize_lon(lon - lon0), y0 - a * k0 * t.ln()))
            }
            Method::TransverseMercator { lat0, lon0, k0, x0, y0 } => {
                let tm = Kruger::new(el);
                let (xi0, _) = tm.forward(lat0, 0.0);
                let (xi, eta) = tm.forward(lat, normalize_lon(lon - lon0));
                Some((x0 + k0 * tm.big_a * eta, y0 + k0 * tm.big_a * (xi - xi0)))
            }
            Method::LambertConformal {
                lat0,
                lon0,
                lat1,
                lat2,
                k0,
                x0,
                y0,
            } => {
                let cone = Lcc::new(el, lat0, lat1, lat2, k0);
                let rho = cone.rho(lat)?;
                let theta = cone.n * normalize_lon(lon - lon0);
                Some((x0 + rho * theta.sin(), y0 + cone.rho0 - rho * theta.cos()))
            }
            Method::LambertAzimuthal { lat0, lon0, x0, y0 } => {
                let q = Qp::new(el);
                let beta1 = q.beta(lat0);
                let d = a * m_of(el, lat0) / (q.rq * beta1.cos());
                let beta = q.beta(lat);
                let dl = normalize_lon(lon - lon0);
                let denominator = 1.0 + beta1.sin() * beta.sin() + beta1.cos() * beta.cos() * dl.cos();
                if denominator <= 1e-12 {
                    return None;
                }
                let b = q.rq * (2.0 / denominator).sqrt();
                Some((
                    x0 + b * d * beta.cos() * dl.sin(),
                    y0 + (b / d) * (beta1.cos() * beta.sin() - beta1.sin() * beta.cos() * dl.cos()),
                ))
            }
            Method::Albers {
                lat0,
                lon0,
                lat1,
                lat2,
                x0,
                y0,
            } => {
                let cone = Aea::new(el, lat0, lat1, lat2);
                let rho = cone.rho(lat)?;
                let theta = cone.n * normalize_lon(lon - lon0);
                Some((x0 + rho * theta.sin(), y0 + cone.rho0 - rho * theta.cos()))
            }
            Method::PolarStereographic {
                north,
                lat_ts,
                lon0,
                k0,
                x0,
                y0,
            } => {
                let stereo = Stereo::new(el, north, lat_ts, k0);
                let (lat, dl) = if north {
                    (lat, normalize_lon(lon - lon0))
                } else {
                    (-lat, -normalize_lon(lon - lon0))
                };
                let rho = stereo.scale * t_of(e, lat);
                let (x, y) = (rho * dl.sin(), -rho * dl.cos());
                Some(if north { (x0 + x, y0 + y) } else { (x0 - x, y0 - y) })
            }
        }
    }

    /// Projected metres to geodetic (radians, this ellipsoid).
    fn inverse(&self, x: f64, y: f64) -> Option<(f64, f64)> {
        let el = self.ellipsoid;
        let (a, e) = (el.a, el.e());
        let (lon, lat) = match self.method {
            Method::LongLat => (x, y),
            Method::Mercator { lon0, k0, x0, y0 } => {
                let t = (-(y - y0) / (a * k0)).exp();
                (lon0 + (x - x0) / (a * k0), phi_of_t(e, t)?)
            }
            Method::TransverseMercator { lat0, lon0, k0, x0, y0 } => {
                let tm = Kruger::new(el);
                let (xi0, _) = tm.forward(lat0, 0.0);
                let xi = (y - y0) / (k0 * tm.big_a) + xi0;
                let eta = (x - x0) / (k0 * tm.big_a);
                let (lat, dl) = tm.inverse(xi, eta)?;
                (lon0 + dl, lat)
            }
            Method::LambertConformal {
                lat0,
                lon0,
                lat1,
                lat2,
                k0,
                x0,
                y0,
            } => {
                let cone = Lcc::new(el, lat0, lat1, lat2, k0);
                let (dx, dy) = (x - x0, cone.rho0 - (y - y0));
                let sign = cone.n.signum();
                let rho = sign * dx.hypot(dy);
                let theta = (sign * dx).atan2(sign * dy);
                let lat = if rho == 0.0 {
                    sign * FRAC_PI_2
                } else {
                    phi_of_t(e, (rho / (a * cone.f * k0)).powf(1.0 / cone.n))?
                };
                (lon0 + theta / cone.n, lat)
            }
            Method::LambertAzimuthal { lat0, lon0, x0, y0 } => {
                let q = Qp::new(el);
                let beta1 = q.beta(lat0);
                let d = a * m_of(el, lat0) / (q.rq * beta1.cos());
                let (dx, dy) = (x - x0, y - y0);
                let rho = (dx / d).hypot(d * dy);
                if rho < 1e-9 {
                    return Some((lon0, lat0));
                }
                let ratio = rho / (2.0 * q.rq);
                if ratio > 1.0 + 1e-12 {
                    return None;
                }
                let ce = 2.0 * ratio.min(1.0).asin();
                let qv = q.qp * (ce.cos() * beta1.sin() + d * dy * ce.sin() * beta1.cos() / rho);
                let dl = (dx * ce.sin()).atan2(d * beta1.cos() * rho * ce.cos() - d * d * dy * beta1.sin() * ce.sin());
                (lon0 + dl, phi_of_q(el, qv)?)
            }
            Method::Albers {
                lat0,
                lon0,
                lat1,
                lat2,
                x0,
                y0,
            } => {
                let cone = Aea::new(el, lat0, lat1, lat2);
                let (dx, dy) = (x - x0, cone.rho0 - (y - y0));
                let sign = cone.n.signum();
                let rho = sign * dx.hypot(dy);
                let theta = (sign * dx).atan2(sign * dy);
                let qv = (cone.c - (rho * cone.n / a).powi(2)) / cone.n;
                (lon0 + theta / cone.n, phi_of_q(el, qv)?)
            }
            Method::PolarStereographic {
                north,
                lat_ts,
                lon0,
                k0,
                x0,
                y0,
            } => {
                let stereo = Stereo::new(el, north, lat_ts, k0);
                let (dx, dy) = if north { (x - x0, y - y0) } else { (x0 - x, y0 - y) };
                let rho = dx.hypot(dy);
                let lat = if rho == 0.0 {
                    FRAC_PI_2
                } else {
                    phi_of_t(e, rho / stereo.scale)?
                };
                let dl = dx.atan2(-dy);
                if north {
                    (lon0 + dl, lat)
                } else {
                    (lon0 - dl, -lat)
                }
            }
        };
        (lat.is_finite() && lon.is_finite()).then_some((lon, lat))
    }
}

/// A longitude brought into -π to π.
fn normalize_lon(lon: f64) -> f64 {
    let wrapped = (lon + PI).rem_euclid(2.0 * PI) - PI;
    if wrapped == -PI && lon > 0.0 {
        PI
    } else {
        wrapped
    }
}

/// m = cos φ / √(1 − e² sin² φ)
fn m_of(el: Ellipsoid, phi: f64) -> f64 {
    phi.cos() / (1.0 - el.e2() * phi.sin().powi(2)).sqrt()
}

/// Snyder's t = tan(π/4 − φ/2) / ((1 − e sin φ)/(1 + e sin φ))^(e/2)
fn t_of(e: f64, phi: f64) -> f64 {
    let es = e * phi.sin();
    (FRAC_PI_4 - phi / 2.0).tan() / ((1.0 - es) / (1.0 + es)).powf(e / 2.0)
}

/// The latitude whose t is `t`, iterated (Snyder 7-9)
fn phi_of_t(e: f64, t: f64) -> Option<f64> {
    if !t.is_finite() || t < 0.0 {
        return None;
    }
    let mut phi = FRAC_PI_2 - 2.0 * t.atan();
    for _ in 0..30 {
        let es = e * phi.sin();
        let next = FRAC_PI_2 - 2.0 * (t * ((1.0 - es) / (1.0 + es)).powf(e / 2.0)).atan();
        if (next - phi).abs() < 1e-14 {
            return Some(next);
        }
        phi = next;
    }
    // not converged: no latitude, rather than a wrong one
    None
}

/// Snyder's q (authalic), and the latitude of a q by iteration
fn q_of(el: Ellipsoid, phi: f64) -> f64 {
    let (e2, e) = (el.e2(), el.e());
    let s = phi.sin();
    if e == 0.0 {
        return 2.0 * s;
    }
    (1.0 - e2) * (s / (1.0 - e2 * s * s) - (1.0 / (2.0 * e)) * ((1.0 - e * s) / (1.0 + e * s)).ln())
}

fn phi_of_q(el: Ellipsoid, q: f64) -> Option<f64> {
    let (e2, e) = (el.e2(), el.e());
    if !q.is_finite() {
        return None;
    }
    if e == 0.0 {
        return Some((q / 2.0).clamp(-1.0, 1.0).asin());
    }
    let qp = q_of(el, FRAC_PI_2);
    if q.abs() > qp + 1e-9 {
        return None;
    }
    if (q.abs() - qp).abs() < 1e-12 {
        return Some(FRAC_PI_2.copysign(q));
    }
    let mut phi = (q / 2.0).clamp(-1.0, 1.0).asin();
    for _ in 0..30 {
        let s = phi.sin();
        let one = 1.0 - e2 * s * s;
        let next = phi
            + one * one / (2.0 * phi.cos())
                * (q / (1.0 - e2) - s / one + (1.0 / (2.0 * e)) * ((1.0 - e * s) / (1.0 + e * s)).ln());
        if !next.is_finite() {
            return None;
        }
        if (next - phi).abs() < 1e-14 {
            return Some(next);
        }
        phi = next;
    }
    None
}

/// The authalic sphere of an ellipsoid (LAEA): qp, its radius and β of a latitude.
struct Qp {
    qp: f64,
    rq: f64,
    el: Ellipsoid,
}

impl Qp {
    fn new(el: Ellipsoid) -> Self {
        let qp = q_of(el, FRAC_PI_2);
        Self {
            qp,
            rq: el.a * (qp / 2.0).sqrt(),
            el,
        }
    }

    fn beta(&self, phi: f64) -> f64 {
        (q_of(self.el, phi) / self.qp).clamp(-1.0, 1.0).asin()
    }
}

/// A Lambert conformal cone: n, F and ρ0 (Snyder 15-1 to 15-3, with k0 for a one-parallel cone)
struct Lcc {
    n: f64,
    f: f64,
    rho0: f64,
    a: f64,
    k0: f64,
    e: f64,
}

impl Lcc {
    fn new(el: Ellipsoid, lat0: f64, lat1: f64, lat2: f64, k0: f64) -> Self {
        let e = el.e();
        let (m1, m2) = (m_of(el, lat1), m_of(el, lat2));
        let (t1, t2) = (t_of(e, lat1), t_of(e, lat2));
        let n = if (lat1 - lat2).abs() < 1e-12 {
            lat1.sin()
        } else {
            (m1.ln() - m2.ln()) / (t1.ln() - t2.ln())
        };
        let f = m1 / (n * t1.powf(n));
        let rho0 = el.a * f * k0 * t_of(e, lat0).powf(n);
        Self {
            n,
            f,
            rho0,
            a: el.a,
            k0,
            e,
        }
    }

    fn rho(&self, phi: f64) -> Option<f64> {
        if (phi.abs() - FRAC_PI_2).abs() < 1e-12 {
            return (phi * self.n > 0.0).then_some(0.0);
        }
        Some(self.a * self.f * self.k0 * t_of(self.e, phi).powf(self.n))
    }
}

/// An Albers cone: n, C and ρ0 (Snyder 14-1 to 14-6)
struct Aea {
    n: f64,
    c: f64,
    rho0: f64,
    a: f64,
    el: Ellipsoid,
}

impl Aea {
    fn new(el: Ellipsoid, lat0: f64, lat1: f64, lat2: f64) -> Self {
        let (m1, m2) = (m_of(el, lat1), m_of(el, lat2));
        let (q1, q2) = (q_of(el, lat1), q_of(el, lat2));
        let n = if (lat1 - lat2).abs() < 1e-12 {
            lat1.sin()
        } else {
            (m1 * m1 - m2 * m2) / (q2 - q1)
        };
        let c = m1 * m1 + n * q1;
        let rho0 = el.a * (c - n * q_of(el, lat0)).max(0.0).sqrt() / n;
        Self {
            n,
            c,
            rho0,
            a: el.a,
            el,
        }
    }

    fn rho(&self, phi: f64) -> Option<f64> {
        let inside = self.c - self.n * q_of(self.el, phi);
        (inside >= 0.0).then(|| self.a * inside.sqrt() / self.n)
    }
}

/// A polar stereographic projection's ρ per t: a·mc/tc with a latitude of true scale, else 2·a·k0/√((1+e)^(1+e)(1−e)^(1−e))
struct Stereo {
    scale: f64,
}

impl Stereo {
    fn new(el: Ellipsoid, north: bool, lat_ts: Option<f64>, k0: f64) -> Self {
        let e = el.e();
        let pole = |k0: f64| 2.0 * el.a * k0 / ((1.0 + e).powf(1.0 + e) * (1.0 - e).powf(1.0 - e)).sqrt();
        let scale = match lat_ts {
            Some(ts) => {
                let ts = if north { ts } else { -ts };
                if (ts - FRAC_PI_2).abs() < 1e-12 {
                    pole(1.0)
                } else {
                    el.a * m_of(el, ts) / t_of(e, ts)
                }
            }
            None => pole(k0),
        };
        Self { scale }
    }
}

/// Krüger's series of the transverse Mercator projection to the sixth order in n (Karney 2011, eqs. 7, 11, 35, 36).
struct Kruger {
    big_a: f64,
    alpha: [f64; 6],
    beta: [f64; 6],
    e: f64,
}

impl Kruger {
    fn new(el: Ellipsoid) -> Self {
        let n = el.f / (2.0 - el.f);
        let (n2, n3, n4, n5, n6) = (n * n, n.powi(3), n.powi(4), n.powi(5), n.powi(6));
        let big_a = el.a / (1.0 + n) * (1.0 + n2 / 4.0 + n4 / 64.0 + n6 / 256.0);
        let alpha = [
            n / 2.0 - 2.0 * n2 / 3.0 + 5.0 * n3 / 16.0 + 41.0 * n4 / 180.0 - 127.0 * n5 / 288.0 + 7891.0 * n6 / 37800.0,
            13.0 * n2 / 48.0 - 3.0 * n3 / 5.0 + 557.0 * n4 / 1440.0 + 281.0 * n5 / 630.0
                - 1_983_433.0 * n6 / 1_935_360.0,
            61.0 * n3 / 240.0 - 103.0 * n4 / 140.0 + 15061.0 * n5 / 26880.0 + 167_603.0 * n6 / 181_440.0,
            49561.0 * n4 / 161_280.0 - 179.0 * n5 / 168.0 + 6_601_661.0 * n6 / 7_257_600.0,
            34729.0 * n5 / 80640.0 - 3_418_889.0 * n6 / 1_995_840.0,
            212_378_941.0 * n6 / 319_334_400.0,
        ];
        let beta = [
            n / 2.0 - 2.0 * n2 / 3.0 + 37.0 * n3 / 96.0 - n4 / 360.0 - 81.0 * n5 / 512.0 + 96199.0 * n6 / 604_800.0,
            n2 / 48.0 + n3 / 15.0 - 437.0 * n4 / 1440.0 + 46.0 * n5 / 105.0 - 1_118_711.0 * n6 / 3_870_720.0,
            17.0 * n3 / 480.0 - 37.0 * n4 / 840.0 - 209.0 * n5 / 4480.0 + 5569.0 * n6 / 90720.0,
            4397.0 * n4 / 161_280.0 - 11.0 * n5 / 504.0 - 830_251.0 * n6 / 7_257_600.0,
            4583.0 * n5 / 161_280.0 - 108_847.0 * n6 / 3_991_680.0,
            20_648_693.0 * n6 / 638_668_800.0,
        ];
        Self {
            big_a,
            alpha,
            beta,
            e: el.e(),
        }
    }

    /// τ′ of τ: the tangent of the conformal latitude
    fn tau_prime(&self, tau: f64) -> f64 {
        let sigma = (self.e * (self.e * tau / (1.0 + tau * tau).sqrt()).atanh()).sinh();
        tau * (1.0 + sigma * sigma).sqrt() - sigma * (1.0 + tau * tau).sqrt()
    }

    /// (ξ, η) of a latitude and a longitude from the central meridian
    fn forward(&self, lat: f64, dl: f64) -> (f64, f64) {
        let tau_p = self.tau_prime(lat.tan());
        let xi_p = tau_p.atan2(dl.cos());
        let eta_p = (dl.sin() / tau_p.hypot(dl.cos())).asinh();
        let mut xi = xi_p;
        let mut eta = eta_p;
        for (j, alpha) in self.alpha.iter().enumerate() {
            let k = 2.0 * (j as f64 + 1.0);
            xi += alpha * (k * xi_p).sin() * (k * eta_p).cosh();
            eta += alpha * (k * xi_p).cos() * (k * eta_p).sinh();
        }
        (xi, eta)
    }

    /// The latitude and the longitude from the central meridian of (ξ, η)
    fn inverse(&self, xi: f64, eta: f64) -> Option<(f64, f64)> {
        let mut xi_p = xi;
        let mut eta_p = eta;
        for (j, beta) in self.beta.iter().enumerate() {
            let k = 2.0 * (j as f64 + 1.0);
            xi_p -= beta * (k * xi).sin() * (k * eta).cosh();
            eta_p -= beta * (k * xi).cos() * (k * eta).sinh();
        }
        let tau_p = xi_p.sin() / eta_p.sinh().hypot(xi_p.cos());
        let dl = eta_p.sinh().atan2(xi_p.cos());
        // τ from τ′ by Newton's method (Karney eq. 19 to 21)
        let e2 = self.e * self.e;
        let mut tau = tau_p;
        for _ in 0..10 {
            let current = self.tau_prime(tau);
            let step = (tau_p - current) / (1.0 + current * current).sqrt() * (1.0 + (1.0 - e2) * tau * tau)
                / ((1.0 - e2) * (1.0 + tau * tau).sqrt());
            tau += step;
            if !tau.is_finite() {
                return None;
            }
            if step.abs() < 1e-15 * tau.abs().max(1.0) {
                break;
            }
        }
        Some((tau.atan(), dl))
    }
}

/// WGS84 (EPSG:4326) and Web Mercator (EPSG:3857), which every map knows, as projections.
#[must_use]
pub fn builtin(code: u32) -> Option<Projection> {
    match code {
        4326 => Projection::parse_proj4("+proj=longlat +datum=WGS84 +no_defs").ok(),
        3857 | 900_913 | 3785 | 102_100 | 102_113 => Projection::parse_proj4(
            "+proj=merc +a=6378137 +b=6378137 +lat_ts=0 +lon_0=0 +x_0=0 +y_0=0 +k=1 +units=m +nadgrids=@null",
        )
        .ok(),
        _ => None,
    }
}

/// The WGS84 box (west, south, east, north, degrees) of a box in a projection's coordinates: its edges sampled (a
/// projected box's edges are curves in degrees), a pole inside it taken in whole. None when no edge point converts.
#[must_use]
pub fn wgs84_bounds(projection: &Projection, [min_x, min_y, max_x, max_y]: [f64; 4]) -> Option<[f64; 4]> {
    const STEPS: usize = 32;
    let mut out: Option<[f64; 4]> = None;
    let mut add = |lon: f64, lat: f64| {
        out = Some(match out {
            None => [lon, lat, lon, lat],
            Some([w, s, e, n]) => [w.min(lon), s.min(lat), e.max(lon), n.max(lat)],
        });
    };
    for step in 0..=STEPS {
        let t = step as f64 / STEPS as f64;
        let x = min_x + (max_x - min_x) * t;
        let y = min_y + (max_y - min_y) * t;
        for (px, py) in [(x, min_y), (x, max_y), (min_x, y), (max_x, y)] {
            if let Some((lon, lat)) = projection.to_wgs84(px, py) {
                add(lon, lat);
            }
        }
    }
    let mut bounds = out?;
    for pole in [90.0, -90.0] {
        if let Some((x, y)) = projection.from_wgs84(0.0, pole) {
            if x >= min_x && x <= max_x && y >= min_y && y <= max_y {
                bounds = [-180.0, bounds[1].min(pole), 180.0, bounds[3].max(pole)];
            }
        }
    }
    Some(bounds)
}

/// A registered CRS: its EPSG code, label, definition read and whether its authority's axis order is northing (or
/// latitude) first.
#[derive(Clone, Debug)]
pub struct Registered {
    pub code: u32,
    pub label: String,
    pub projection: Projection,
    pub north_first: bool,
}

/// The registry of projections a reader may convert from: the rows that read, by EPSG code.
#[derive(Clone, Debug, Default)]
pub struct Registry {
    entries: BTreeMap<u32, Registered>,
    /// rows whose definition the server does not read, with why: a map in the browser may still draw them
    unreadable: BTreeMap<u32, String>,
}

impl Registry {
    /// An empty registry: only WGS84 and Web Mercator, which every reader knows.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Add a row: `code` as `EPSG:32633` (or the bare number), its PROJ.4 definition and axis order (`neu` for northing
    /// first). A row that does not read is the error, and nothing is added.
    ///
    /// # Errors
    /// The code or the definition that does not read, in words.
    pub fn add(&mut self, code: &str, label: &str, definition: &str, axis_order: &str) -> Result<(), String> {
        let number = epsg_number(code).ok_or_else(|| format!("{code} is not an EPSG code"))?;
        let projection = match Projection::parse_proj4(definition) {
            Ok(projection) => projection,
            Err(err) => {
                self.unreadable.insert(number, err.clone());
                return Err(format!("{code}: {err}"));
            }
        };
        let north_first = axis_order.trim().eq_ignore_ascii_case("neu")
            || (projection.is_geographic() && !axis_order.trim().eq_ignore_ascii_case("enu"));
        self.entries.insert(
            number,
            Registered {
                code: number,
                label: label.to_string(),
                projection,
                north_first,
            },
        );
        Ok(())
    }

    #[must_use]
    pub fn get(&self, code: u32) -> Option<&Registered> {
        self.entries.get(&code)
    }

    /// Why a registered row's definition is not read on the server (it was added and refused), or None.
    #[must_use]
    pub fn unreadable(&self, code: u32) -> Option<&str> {
        self.entries
            .get(&code)
            .map_or_else(|| self.unreadable.get(&code).map(String::as_str), |_| None)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Registered> {
        self.entries.values()
    }

    /// The registered CRS a WKT1 or WKT2 definition (a shapefile's `.prj`) names: by the EPSG code it gives at its top
    /// level, else by the name ESRI writes for a common one (`WGS_1984_UTM_Zone_33N`, `British_National_Grid`), else
    /// by its parameters matching a row's. None when none matches.
    #[must_use]
    pub fn find_wkt(&self, wkt: &str) -> Option<&Registered> {
        if let Some(code) = wkt_authority(wkt) {
            return self.get(code);
        }
        if let Some(code) = esri_name_code(wkt) {
            if let Some(found) = self.get(code) {
                return Some(found);
            }
        }
        let parsed = wkt_projection(wkt)?;
        self.entries
            .values()
            .find(|entry| same_projection(&entry.projection, &parsed))
    }
}

/// The number of `EPSG:n`, `urn:ogc:def:crs:EPSG::n`, `http://www.opengis.net/def/crs/EPSG/0/n` or a bare number.
#[must_use]
pub fn epsg_number(code: &str) -> Option<u32> {
    let upper = code.trim().to_ascii_uppercase();
    if !(upper.contains("EPSG") || upper.chars().all(|c| c.is_ascii_digit())) {
        return None;
    }
    upper
        .rsplit([':', '/', '#'])
        .find(|part| !part.is_empty())
        .and_then(|part| part.parse().ok())
        .filter(|code| *code > 0)
}

/// The EPSG code of a WKT's outermost AUTHORITY["EPSG","n"] (WKT1, the last one) or ID["EPSG",n] (WKT2).
fn wkt_authority(wkt: &str) -> Option<u32> {
    let upper: String = wkt
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_uppercase();
    let trimmed = upper.trim_end_matches(']');
    for marker in ["AUTHORITY[\"EPSG\",\"", "ID[\"EPSG\","] {
        if let Some(at) = trimmed.rfind(marker) {
            // the outermost: nothing but closing brackets after it
            let rest = &trimmed[at + marker.len()..];
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            let after = &rest[digits.len()..];
            if !digits.is_empty() && after.trim_start_matches('"').is_empty() {
                return digits.parse().ok();
            }
        }
    }
    None
}

/// The EPSG code of the names ESRI's `.prj` writes for the registry's common CRSs.
fn esri_name_code(wkt: &str) -> Option<u32> {
    let name = wkt.split('"').nth(1)?.to_ascii_uppercase().replace(' ', "_");
    let zone = |prefix: &str| -> Option<(u32, bool)> {
        let rest = name.strip_prefix(prefix)?;
        let (digits, hemisphere) = rest.split_at(rest.len().checked_sub(1)?);
        Some((digits.parse().ok()?, hemisphere == "N"))
    };
    if let Some((zone, north)) = zone("WGS_1984_UTM_ZONE_") {
        return (1..=60)
            .contains(&zone)
            .then_some(if north { 32600 } else { 32700 } + zone);
    }
    if let Some((zone, true)) = zone("NAD_1983_UTM_ZONE_") {
        return (1..=23).contains(&zone).then_some(26900 + zone);
    }
    match name.as_str() {
        "BRITISH_NATIONAL_GRID" | "OSGB_1936_BRITISH_NATIONAL_GRID" | "OSGB36_/_BRITISH_NATIONAL_GRID" => Some(27700),
        "RGF_1993_LAMBERT_93" | "RGF93_/_LAMBERT-93" | "RGF93_V1_/_LAMBERT-93" => Some(2154),
        "ETRS_1989_LAEA" | "ETRS89_/_LAEA_EUROPE" | "ETRS89-EXTENDED_/_LAEA_EUROPE" => Some(3035),
        "GCS_NORTH_AMERICAN_1983" | "NAD83" => Some(4269),
        "NAD_1983_CONTIGUOUS_USA_ALBERS"
        | "USA_CONTIGUOUS_ALBERS_EQUAL_AREA_CONIC_USGS_VERSION"
        | "NAD83_/_CONUS_ALBERS" => Some(5070),
        _ => None,
    }
}

/// A WKT1 PROJCS's method, ellipsoid and parameters as a [`Projection`] (no datum shift is read from it: a match by
/// parameters takes the registered row's).
fn wkt_projection(wkt: &str) -> Option<Projection> {
    let upper = wkt.to_ascii_uppercase();
    if !upper.trim_start().starts_with("PROJCS") {
        return None;
    }
    let quoted_after = |marker: &str| -> Option<String> {
        let at = upper.find(marker)? + marker.len();
        upper[at..].split('"').nth(1).map(str::to_string)
    };
    let parameter = |name: &str| -> Option<f64> {
        let mut rest = upper.as_str();
        while let Some(at) = rest.find("PARAMETER[\"") {
            rest = &rest[at + "PARAMETER[\"".len()..];
            let (found, after) = rest.split_once('"')?;
            if found.replace(' ', "_") == name {
                let value = after.trim_start_matches(',').split([']', ',']).next()?.trim();
                return value.parse().ok();
            }
        }
        None
    };
    let spheroid = {
        let at = upper.find("SPHEROID[").or_else(|| upper.find("ELLIPSOID["))?;
        let mut parts = upper[at..].split(',').skip(1);
        let a: f64 = parts.next()?.trim().parse().ok()?;
        let rf: f64 = parts.next()?.trim().trim_end_matches(']').parse().ok()?;
        Ellipsoid {
            a,
            f: if rf == 0.0 { 0.0 } else { 1.0 / rf },
            name: "a given ellipsoid",
        }
    };
    let unit = {
        // the last UNIT of a PROJCS is its linear unit
        let at = upper.rfind("UNIT[")?;
        upper[at..]
            .split(',')
            .nth(1)?
            .trim()
            .trim_end_matches(']')
            .parse::<f64>()
            .ok()?
    };
    let degrees = |name: &str| parameter(name).unwrap_or(0.0).to_radians();
    let x0 = parameter("FALSE_EASTING").unwrap_or(0.0) * unit;
    let y0 = parameter("FALSE_NORTHING").unwrap_or(0.0) * unit;
    let method_name = quoted_after("PROJECTION[")?.replace(' ', "_");
    let lat0 = if parameter("LATITUDE_OF_ORIGIN").is_some() {
        degrees("LATITUDE_OF_ORIGIN")
    } else {
        degrees("LATITUDE_OF_CENTER")
    };
    let lon0 = if parameter("CENTRAL_MERIDIAN").is_some() {
        degrees("CENTRAL_MERIDIAN")
    } else {
        degrees("LONGITUDE_OF_CENTER")
    };
    let k0 = parameter("SCALE_FACTOR").unwrap_or(1.0);
    let method = match method_name.as_str() {
        "TRANSVERSE_MERCATOR" | "GAUSS_KRUGER" => Method::TransverseMercator { lat0, lon0, k0, x0, y0 },
        "LAMBERT_CONFORMAL_CONIC" | "LAMBERT_CONFORMAL_CONIC_2SP" | "LAMBERT_CONFORMAL_CONIC_1SP" => {
            let lat1 = parameter("STANDARD_PARALLEL_1").map_or(lat0, f64::to_radians);
            Method::LambertConformal {
                lat0,
                lon0,
                lat1,
                lat2: parameter("STANDARD_PARALLEL_2").map_or(lat1, f64::to_radians),
                k0,
                x0,
                y0,
            }
        }
        "LAMBERT_AZIMUTHAL_EQUAL_AREA" => Method::LambertAzimuthal { lat0, lon0, x0, y0 },
        "ALBERS" | "ALBERS_CONIC_EQUAL_AREA" => {
            let lat1 = degrees("STANDARD_PARALLEL_1");
            Method::Albers {
                lat0,
                lon0,
                lat1,
                lat2: parameter("STANDARD_PARALLEL_2").map_or(lat1, f64::to_radians),
                x0,
                y0,
            }
        }
        _ => return None,
    };
    Some(Projection {
        method,
        ellipsoid: spheroid,
        to_wgs84: None,
        to_meter: unit,
        datum: String::new(),
        zero_shift_given: false,
    })
}

/// Whether two projections are the same method with the same parameters and ellipsoid (to a millimetre and a
/// micro-degree), whatever their datum shifts.
fn same_projection(registered: &Projection, parsed: &Projection) -> bool {
    let close = |a: f64, b: f64, tolerance: f64| (a - b).abs() <= tolerance;
    let angle = |a: f64, b: f64| close(a, b, 1e-8);
    let metre = |a: f64, b: f64| close(a, b, 1e-3);
    if !metre(registered.ellipsoid.a, parsed.ellipsoid.a)
        || !close(registered.ellipsoid.f, parsed.ellipsoid.f, 1e-9)
        || !close(registered.to_meter, parsed.to_meter, 1e-9)
    {
        return false;
    }
    match (&registered.method, &parsed.method) {
        (
            Method::TransverseMercator {
                lat0: a0,
                lon0: b0,
                k0: c0,
                x0: d0,
                y0: e0,
            },
            Method::TransverseMercator {
                lat0: a1,
                lon0: b1,
                k0: c1,
                x0: d1,
                y0: e1,
            },
        ) => angle(*a0, *a1) && angle(*b0, *b1) && close(*c0, *c1, 1e-10) && metre(*d0, *d1) && metre(*e0, *e1),
        (
            Method::LambertConformal {
                lat0: a0,
                lon0: b0,
                lat1: c0,
                lat2: d0,
                x0: e0,
                y0: f0,
                ..
            },
            Method::LambertConformal {
                lat0: a1,
                lon0: b1,
                lat1: c1,
                lat2: d1,
                x0: e1,
                y0: f1,
                ..
            },
        ) => {
            angle(*a0, *a1)
                && angle(*b0, *b1)
                && ((angle(*c0, *c1) && angle(*d0, *d1)) || (angle(*c0, *d1) && angle(*d0, *c1)))
                && metre(*e0, *e1)
                && metre(*f0, *f1)
        }
        (
            Method::LambertAzimuthal {
                lat0: a0,
                lon0: b0,
                x0: c0,
                y0: d0,
            },
            Method::LambertAzimuthal {
                lat0: a1,
                lon0: b1,
                x0: c1,
                y0: d1,
            },
        ) => angle(*a0, *a1) && angle(*b0, *b1) && metre(*c0, *c1) && metre(*d0, *d1),
        (
            Method::Albers {
                lat0: a0,
                lon0: b0,
                lat1: c0,
                lat2: d0,
                x0: e0,
                y0: f0,
            },
            Method::Albers {
                lat0: a1,
                lon0: b1,
                lat1: c1,
                lat2: d1,
                x0: e1,
                y0: f1,
            },
        ) => {
            angle(*a0, *a1)
                && angle(*b0, *b1)
                && ((angle(*c0, *c1) && angle(*d0, *d1)) || (angle(*c0, *d1) && angle(*d0, *c1)))
                && metre(*e0, *e1)
                && metre(*f0, *f1)
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "proj_tests.rs"]
mod tests;
