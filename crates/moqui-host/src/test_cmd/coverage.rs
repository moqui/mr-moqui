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

//! The service coverage report of `moqui test --coverage`: the services a component's `service/**/*.xml` files
//! define, set against the services the test run called (`moqui_core::service::coverage`).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use moqui_lib::node::MNode;

/// The services of one service file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCoverage {
    /// The file under the component, such as `service/mantle/order/OrderServices.xml`.
    pub file: String,
    /// Each defined service and how many times the run called it.
    pub services: Vec<(String, u64)>,
}

impl FileCoverage {
    fn called(&self) -> usize {
        self.services.iter().filter(|(_, count)| *count > 0).count()
    }
}

/// The service files that are not service definitions: REST mappings and ECA rule files.
fn is_definition_file(name: &str) -> bool {
    name.ends_with(".xml")
        && ![".rest.xml", ".secas.xml", ".eecas.xml", ".emecas.xml", ".gitecas.xml"]
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if is_definition_file(&path.file_name().unwrap_or_default().to_string_lossy()) {
            out.push(path);
        }
    }
}

/// The calls made, by the services each file of `component_dir/service` defines. Interface services are not
/// callable and are left out.
pub fn coverage(component_dir: &Path, counts: &BTreeMap<String, u64>) -> Vec<FileCoverage> {
    let service_dir = component_dir.join("service");
    let mut files = Vec::new();
    collect(&service_dir, &mut files);
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let Ok(root) = MNode::parse_file(&path) else {
            continue;
        };
        if !root.has_name("services") {
            continue;
        }
        let relative = path.strip_prefix(&service_dir).unwrap_or(&path);
        // A service location uses `/` on every platform. `display` would write `\` on Windows.
        let relative_name = relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let package = relative
            .with_extension("")
            .components()
            .map(|part| part.as_os_str().to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(".");
        let mut services = Vec::new();
        for service in root.named_iter("service") {
            if service.attribute_is("type", "interface") {
                continue;
            }
            let verb = service.attribute("verb").unwrap_or_default();
            let noun = service.attribute("noun").unwrap_or_default();
            let name = if noun.is_empty() {
                format!("{package}.{verb}")
            } else {
                format!("{package}.{verb}#{noun}")
            };
            let count = counts.get(&name).copied().unwrap_or(0);
            services.push((name, count));
        }
        if !services.is_empty() {
            out.push(FileCoverage {
                file: format!("service/{relative_name}"),
                services,
            });
        }
    }
    out
}

/// The report: a line per file with the services called out of the services defined, the services not called, and the
/// total. With `full` the called services are listed with their counts too.
pub fn render(component: &str, files: &[FileCoverage], full: bool) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "Service coverage: {component}");
    let (mut called, mut total) = (0, 0);
    for file in files {
        called += file.called();
        total += file.services.len();
        let _ = writeln!(out, "{:<56} {:>3}/{:<3}", file.file, file.called(), file.services.len());
        if full {
            for (name, count) in file.services.iter().filter(|(_, count)| *count > 0) {
                let _ = writeln!(out, "    called {count:>5}  {name}");
            }
        }
        for (name, _) in file.services.iter().filter(|(_, count)| *count == 0) {
            let _ = writeln!(out, "    not called     {name}");
        }
    }
    let _ = writeln!(
        out,
        "Total: {called} of {total} services called ({:.0}%)",
        if total == 0 {
            0.0
        } else {
            called as f64 * 100.0 / total as f64
        }
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_names_services_by_file_and_leaves_out_interfaces() {
        let dir = std::env::temp_dir().join(format!("moqui-coverage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("service/acme/shop")).unwrap();
        std::fs::write(
            dir.join("service/acme/shop/OrderServices.xml"),
            r#"<services>
                <service verb="create" noun="Order"/>
                <service verb="ping"/>
                <service verb="shape" noun="Order" type="interface"/>
            </services>"#,
        )
        .unwrap();
        std::fs::write(dir.join("service/acme.rest.xml"), "<resource name=\"x\"/>").unwrap();
        std::fs::write(dir.join("service/Shop.secas.xml"), "<secas/>").unwrap();
        let counts = BTreeMap::from([("acme.shop.OrderServices.create#Order".to_string(), 3)]);
        let files = coverage(&dir, &counts);
        assert_eq!(
            files,
            vec![FileCoverage {
                file: "service/acme/shop/OrderServices.xml".to_string(),
                services: vec![
                    ("acme.shop.OrderServices.create#Order".to_string(), 3),
                    ("acme.shop.OrderServices.ping".to_string(), 0),
                ],
            }]
        );
        let text = render("acme", &files, false);
        assert!(text.contains("service/acme/shop/OrderServices.xml"), "{text}");
        assert!(text.contains("not called     acme.shop.OrderServices.ping"), "{text}");
        assert!(text.contains("Total: 1 of 2 services called (50%)"), "{text}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
