//! GraphQL documents and the `GitHub` methods that send them. Each submodule
//! lists the documents it sends in `DOCUMENTS` so the schema gate sees them.

mod mutations;
mod pull_request;
mod reads;

pub(crate) fn documents() -> Vec<(&'static str, &'static str)> {
    let mut all = vec![("viewer", crate::client::VIEWER_QUERY)];
    all.extend_from_slice(reads::DOCUMENTS);
    all.extend_from_slice(pull_request::DOCUMENTS);
    all.extend_from_slice(mutations::DOCUMENTS);
    all.extend_from_slice(crate::rest::DOCUMENTS);
    all.extend_from_slice(crate::suggestion::DOCUMENTS);
    all.extend_from_slice(crate::workflow::DOCUMENTS);
    all
}
