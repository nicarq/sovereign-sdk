pub(crate) mod files {
    use std::fs::File;
    use std::path::{Path, PathBuf};

    use anyhow::Context;
    use celestia_types::nmt::Namespace;
    use celestia_types::{ExtendedDataSquare, ExtendedHeader, NamespacedShares};
    use serde::de::DeserializeOwned;

    use crate::types::{FilteredCelestiaBlock, NamespaceWithShares};

    pub const ROLLUP_NAMESPACE: Namespace = Namespace::const_v0(*b"\0\0sov-test");

    pub const HEADER_JSON: &str = "header.json";
    pub const ROLLUP_ROWS_JSON: &str = "rollup_rows.json";
    pub const ETX_ROWS_JSON: &str = "etx_rows.json";
    pub const EDS_JSON: &str = "eds.json";

    pub mod with_rollup_data {
        use super::*;
        pub const DATA_PATH: &str = "test_data/block_with_rollup_data";

        pub fn filtered_block() -> FilteredCelestiaBlock {
            let path = make_test_path(DATA_PATH);
            filtered_block_from_path(ROLLUP_NAMESPACE, &path).unwrap()
        }
    }

    pub mod without_rollup_data {
        use super::*;
        const DATA_PATH: &str = "test_data/block_without_rollup_data";

        pub fn filtered_block() -> FilteredCelestiaBlock {
            let path = make_test_path(DATA_PATH);
            filtered_block_from_path(ROLLUP_NAMESPACE, &path).unwrap()
        }
    }

    fn filtered_block_from_path(
        ns: Namespace,
        path: &Path,
    ) -> anyhow::Result<FilteredCelestiaBlock> {
        let header: ExtendedHeader = load_from_file(path, HEADER_JSON)?;
        let rollup_rows: NamespacedShares = load_from_file(path, ROLLUP_ROWS_JSON)?;
        let etx_rows: NamespacedShares = load_from_file(path, ETX_ROWS_JSON)?;
        let eds: ExtendedDataSquare = load_from_file(path, EDS_JSON)?;

        let rollup_batch_data = NamespaceWithShares {
            namespace: ns,
            rows: rollup_rows,
        };

        FilteredCelestiaBlock::new(rollup_batch_data, header, etx_rows, eds)
    }

    pub(crate) fn make_test_path(data_path: &str) -> PathBuf {
        let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        path.push(Path::new(data_path));
        path
    }

    pub(crate) fn load_from_file<T: DeserializeOwned>(
        path: &Path,
        name: &str,
    ) -> anyhow::Result<T> {
        let path = path.join(name);
        let file = File::open(&path).context(format!("Failed to open {}", path.display()))?;
        serde_json::from_reader(file).context(format!("Failed to parse {}", path.display()))
    }
}
