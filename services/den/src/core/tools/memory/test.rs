#[cfg(test)]
mod apply_core_update_projection_tests {
    include!("../tests/apply_core_update_projection.rs");
}

#[cfg(test)]
mod request_review_projection_tests {
    include!("../tests/memory_request_review_projection.rs");
}

#[cfg(test)]
mod request_review_bound_tests {
    include!("../tests/memory_request_review_bound.rs");
}

#[cfg(test)]
mod resolve_proposal_projection_tests {
    include!("../tests/memory_resolve_proposal_projection.rs");
}
