use serde_with::serde_as;

use super::axum_extractors::QueryStringValidation;

/// Query parameters that specify cursor-based pagination for a collection of
/// entities.
///
/// Read more about the tradeoffs of cursor-based VS offset-based pagination in
/// this great article: <https://slack.engineering/evolving-api-pagination-at-slack/>.
// `serde_as` is a workaround for this Serde bug:
// <https://docs.rs/serde_qs/0.12.0/serde_qs/index.html#flatten-workaround>
#[serde_as]
#[derive(Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
pub struct Pagination<T> {
    /// The maximum allowed number of entities to return at once.
    #[serde(default = "pagination_sizes::default", rename = "page[size]")]
    #[serde_as(as = "serde_with::DisplayFromStr")]
    pub size: u32,
    /// See [`PageSelection`].
    #[serde(default, rename = "page")]
    pub selection: PageSelection,
    /// The page cursor, which specifies "where" the page starts within the
    /// collection.
    ///
    /// The cursor is incompatible with first/last pages and mandatory for
    /// next.
    #[serde(default = "Option::default", rename = "page[cursor]")]
    pub cursor: Option<T>,
}

impl<T> QueryStringValidation for Pagination<T> {
    fn validate(&self) -> anyhow::Result<()> {
        // Bad page sizes.
        if self.size == 0 || self.size > pagination_sizes::max() {
            anyhow::bail!(
                "Page size must be between 1 and {}",
                pagination_sizes::max()
            );
        }

        match (&self.cursor, &self.selection) {
            (None, PageSelection::Next) => {
                anyhow::bail!("cursor is required for next page");
            }
            (Some(_), PageSelection::First | PageSelection::Last) => {
                anyhow::bail!("cursor is incompatible with first/last page");
            }
            _ => {}
        }

        Ok(())
    }
}

/// What kind of page a client can request.
#[derive(Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[cfg_attr(feature = "arbitrary", derive(proptest_derive::Arbitrary))]
#[serde(rename_all = "camelCase")]
pub enum PageSelection {
    /// The next page of the collection. A request for the next page will
    /// require a cursor.
    #[default]
    Next,
    /// The first page of the collection.
    First,
    /// The last page of the collection.
    Last,
}

/// Default and max. page sizes; public for testing. They are functions and not
/// constants because of <https://github.com/serde-rs/serde/issues/368>.
#[doc(hidden)]
pub mod pagination_sizes {
    /// The default page size. Less items may be returned if there's not enough
    /// remaining items in the collection.
    pub const fn default() -> u32 {
        25
    }

    /// The maximum allowed page size.
    pub const fn max() -> u32 {
        250
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use proptest::proptest;

    use super::*;
    use crate::axum_extractors::ValidatedQuery;
    use crate::test_utils::uri_with_query_params;

    proptest! {
        #[test]
        fn serialization_roundtrip_equality(sorting: Pagination<String>) {
            let serialized = serde_urlencoded::to_string(&sorting)?;
            let deserialized: Pagination<String> = serde_urlencoded::from_str(&serialized)?;
            assert_eq!(sorting, deserialized);
        }
    }

    fn try_deserialize(query_params: &[(&str, &str)]) -> anyhow::Result<Pagination<String>> {
        let uri = uri_with_query_params(query_params);
        let validated_query = ValidatedQuery::<Pagination<String>>::try_from_uri(&uri)
            // The query rejection type is not a valid error, so we replace it with a dummy error type.
            .map_err(|_| anyhow!("error"))?;

        Ok(validated_query.0)
    }

    #[test]
    fn ok_cases() {
        try_deserialize(&[
            ("page[size]", "10"),
            ("page[cursor]", "foobar"),
            ("page[selection]", "next"),
        ])
        .unwrap();
    }

    #[test]
    fn bad_page_size() {
        try_deserialize(&[("page[size]", "-10")]).unwrap_err();
        try_deserialize(&[("page[size]", "0")]).unwrap_err();
        try_deserialize(&[("page[size]", "100000")]).unwrap_err();
    }

    #[test]
    fn cursor_with_next_is_mandatory() {
        try_deserialize(&[("page", "next"), ("page[cursor]", "foo")]).unwrap();
        try_deserialize(&[("page", "next")]).unwrap_err();
    }

    #[test]
    fn cursor_with_first_and_last_not_ok() {
        try_deserialize(&[("page", "first"), ("page[cursor]", "foo")]).unwrap_err();
        try_deserialize(&[("page", "last"), ("page[cursor]", "foo")]).unwrap_err();
    }
}
