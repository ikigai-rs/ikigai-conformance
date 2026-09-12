//! Parsing a declared RDF face and reading it for the two graph checks: blank
//! nodes ([`Check::SkolemRdf`](crate::Check::SkolemRdf)) and the terms it uses
//! ([`Check::Vocabulary`](crate::Check::Vocabulary)).
//!
//! **This module is public on purpose.** [`parse`] + [`terms`] + [`is_defined`] are
//! exactly what `VOCABULARY` runs, so a module can reproduce the check by hand over
//! any bytes — which is the precise escape hatch for a face whose undefined terms
//! are known and owned elsewhere:
//!
//! ```
//! use ikigai_conformance::rdf::{is_defined, parse, terms};
//!
//! let face = b"@prefix ik: <https://ikigai-rs.dev/ns#> .\n\
//!              <urn:example:review> a ik:Endpoint ; ik:quote \"a quoted line\" .";
//! let undefined: Vec<String> = terms(&parse("text/turtle", face).unwrap())
//!     .into_iter()
//!     .filter(|t| !is_defined(t, &[]))
//!     .collect();
//! // Pinned as an EXACT list, so it goes red in both directions: a NEW invented
//! // term fails, and so does the day `ik:quote` lands in the vocabulary.
//! assert_eq!(undefined, ["https://ikigai-rs.dev/ns#quote"]);
//! ```
//!
//! Prefer that to [`Suite::namespace`](crate::Suite::namespace) when the terms are
//! the module's own and temporarily undefined: registering a namespace waives every
//! term under it forever, including the next one somebody invents.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use oxrdf::vocab::rdf;
use oxrdf::{NamedOrBlankNode, Term, Triple};
use oxrdfio::{RdfFormat, RdfParser};

/// The media types this crate treats as RDF faces. An action declaring one of these
/// as an output is resolved and its result parsed, in [`Check::SkolemRdf`](crate::Check::SkolemRdf).
/// Parameters (`;charset=utf-8`) are ignored when matching.
pub const RDF_FACES: &[&str] = &[
    "text/turtle",
    "application/ld+json",
    "application/rdf+xml",
    "application/n-triples",
    "application/n-quads",
    "application/trig",
];

/// The namespaces a graph face may use freely beside `ikigai-vocab`'s own — the
/// shared vocabularies the module recipe names, plus the RDF/OWL/XSD core. Anything
/// else is "an invented term with no definition" unless the module registers the
/// namespace as its own ([`Suite::namespace`](crate::Suite::namespace)).
///
/// ```
/// use ikigai_conformance::rdf::{is_well_known, WELL_KNOWN_NAMESPACES};
///
/// assert!(WELL_KNOWN_NAMESPACES.contains(&"http://www.w3.org/ns/prov#"));
/// assert!(is_well_known("http://purl.org/dc/terms/created"));
/// assert!(is_well_known("https://schema.org/Person"));
/// assert!(is_well_known("http://schema.org/Person"));
/// assert!(!is_well_known("urn:example:ns#invented"));
/// ```
pub const WELL_KNOWN_NAMESPACES: &[&str] = &[
    "http://www.w3.org/1999/02/22-rdf-syntax-ns#",
    "http://www.w3.org/2000/01/rdf-schema#",
    "http://www.w3.org/2001/XMLSchema#",
    "http://www.w3.org/2002/07/owl#",
    "http://purl.org/dc/terms/",
    "http://xmlns.com/foaf/0.1/",
    "https://schema.org/",
    "http://schema.org/",
    "http://www.w3.org/ns/prov#",
    "http://www.w3.org/2002/12/cal/ical#",
    "http://www.w3.org/2004/02/skos/core#",
    "http://www.w3.org/ns/shacl#",
];

/// Whether `iri` lies under one of [`WELL_KNOWN_NAMESPACES`].
pub fn is_well_known(iri: &str) -> bool {
    WELL_KNOWN_NAMESPACES.iter().any(|ns| iri.starts_with(ns))
}

/// The media type of `repr_type` without its parameters, lower-cased — what
/// [`RDF_FACES`] and a declared output are compared on.
pub fn bare_media_type(media_type: &str) -> String {
    media_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// Whether `media_type` (parameters ignored) is one of [`RDF_FACES`].
pub fn is_rdf_face(media_type: &str) -> bool {
    let bare = bare_media_type(media_type);
    RDF_FACES.contains(&bare.as_str())
}

/// Parse `bytes` as the RDF serialization `media_type` names, into triples (a
/// quad's graph name is dropped — the checks are about terms and nodes, not graphs).
///
/// ```
/// use ikigai_conformance::rdf::parse;
///
/// let triples = parse("text/turtle;charset=utf-8", b"<urn:a> <urn:p> \"x\" .").unwrap();
/// assert_eq!(triples.len(), 1);
/// assert!(parse("text/turtle", b"<urn:a> <urn:p> ").is_err());
/// assert!(parse("text/plain", b"not rdf").is_err(), "not an RDF media type");
/// ```
pub fn parse(media_type: &str, bytes: &[u8]) -> Result<Vec<Triple>, String> {
    let bare = bare_media_type(media_type);
    let format = RdfFormat::from_media_type(&bare)
        .ok_or_else(|| format!("`{bare}` is not an RDF media type this crate can parse"))?;
    let mut triples = Vec::new();
    for quad in RdfParser::from_format(format).for_slice(bytes) {
        let quad = quad.map_err(|e| format!("does not parse as {bare}: {e}"))?;
        triples.push(Triple::new(quad.subject, quad.predicate, quad.object));
    }
    Ok(triples)
}

/// The blank nodes in `triples`, as their `_:label` forms, deduplicated. A
/// skolemized graph has none.
///
/// The recipe's point: a blank node has no identity across two serializations, so a
/// graph with one cannot be diffed, unioned, signed, or addressed by SPARQL by the
/// node's name. The fix is a stable IRI minted from what the node is about —
/// `urn:ikigai:endpoint:{id}:input:{name}` for an endpoint's input, `urn:event:{uid}`
/// for a calendar event, `urn:plan:{name}:step:{n}` for a plan step — never a
/// counter.
///
/// ```
/// use ikigai_conformance::rdf::{blank_nodes, parse};
///
/// let skolem = parse("text/turtle", b"<urn:ikigai:endpoint:cal> <urn:p> <urn:ikigai:endpoint:cal:input:start> .").unwrap();
/// assert!(blank_nodes(&skolem).is_empty());
///
/// let anonymous = parse("text/turtle", b"<urn:ikigai:endpoint:cal> <urn:p> [ <urn:q> \"x\" ] .").unwrap();
/// assert_eq!(blank_nodes(&anonymous).len(), 1);
/// ```
pub fn blank_nodes(triples: &[Triple]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for t in triples {
        if let NamedOrBlankNode::BlankNode(b) = &t.subject {
            found.insert(b.to_string());
        }
        if let Term::BlankNode(b) = &t.object {
            found.insert(b.to_string());
        }
    }
    found
}

/// The vocabulary terms `triples` use: every predicate IRI, plus every IRI in
/// object position of `rdf:type` (a class). Subjects and other objects are data,
/// not vocabulary, and are not returned.
///
/// ```
/// use ikigai_conformance::rdf::{parse, terms};
///
/// let g = parse("text/turtle", b"<urn:x> a <urn:ns#Thing> ; <urn:ns#name> \"x\" ; <urn:ns#sees> <urn:y> .").unwrap();
/// let used = terms(&g);
/// assert!(used.contains("urn:ns#Thing"), "a class");
/// assert!(used.contains("urn:ns#name") && used.contains("urn:ns#sees"), "predicates");
/// assert!(!used.contains("urn:y"), "an object that is not a class is data");
/// ```
pub fn terms(triples: &[Triple]) -> BTreeSet<String> {
    let mut used = BTreeSet::new();
    for t in triples {
        used.insert(t.predicate.as_str().to_string());
        if t.predicate.as_ref() == rdf::TYPE {
            if let Term::NamedNode(class) = &t.object {
                used.insert(class.as_str().to_string());
            }
        }
    }
    used
}

/// The terms `ikigai-vocab` defines: every subject of
/// [`ikigai_vocab::VOCABULARY`] under [`ikigai_vocab::NS`]. Parsed once.
pub fn vocabulary_terms() -> &'static BTreeSet<String> {
    static TERMS: OnceLock<BTreeSet<String>> = OnceLock::new();
    TERMS.get_or_init(|| {
        let triples = parse("text/turtle", ikigai_vocab::VOCABULARY.as_bytes())
            .expect("ikigai-vocab's own vocabulary.ttl parses");
        triples
            .iter()
            .filter_map(|t| match &t.subject {
                NamedOrBlankNode::NamedNode(n) if n.as_str().starts_with(ikigai_vocab::NS) => {
                    Some(n.as_str().to_string())
                }
                _ => None,
            })
            .collect()
    })
}

/// Whether `term` is accounted for: defined in `ikigai-vocab`, under a well-known
/// namespace, or under one of the module's registered `namespaces`.
///
/// An `ik:` term the vocabulary does not define is NOT accounted for by being under
/// `ik:` — that is exactly the invented-term case.
pub fn is_defined(term: &str, namespaces: &[String]) -> bool {
    if term.starts_with(ikigai_vocab::NS) {
        return vocabulary_terms().contains(term);
    }
    is_well_known(term) || namespaces.iter().any(|ns| term.starts_with(ns.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_defines_the_terms_the_catalog_emits() {
        let terms = vocabulary_terms();
        for t in ["Endpoint", "Action", "id", "input", "requires", "output"] {
            assert!(
                terms.contains(&format!("{}{t}", ikigai_vocab::NS)),
                "ik:{t} missing"
            );
        }
        assert!(!terms.contains(&format!("{}invented", ikigai_vocab::NS)));
    }

    #[test]
    fn an_undefined_ik_term_is_not_defined_even_though_it_is_under_ik() {
        assert!(is_defined(&format!("{}Endpoint", ikigai_vocab::NS), &[]));
        assert!(!is_defined(&format!("{}invented", ikigai_vocab::NS), &[]));
        assert!(is_defined("urn:my:ns#x", &["urn:my:ns#".to_string()]));
        assert!(!is_defined("urn:my:ns#x", &[]));
    }

    #[test]
    fn every_rdf_face_has_a_parser() {
        for face in RDF_FACES {
            assert!(RdfFormat::from_media_type(face).is_some(), "{face}");
            assert!(is_rdf_face(&format!("{face};charset=utf-8")));
        }
        assert!(!is_rdf_face("text/plain;charset=utf-8"));
    }

    #[test]
    fn json_ld_and_ntriples_parse_too() {
        let nt = parse("application/n-triples", b"<urn:a> <urn:p> <urn:b> .\n").unwrap();
        assert_eq!(nt.len(), 1);
        let jsonld = parse(
            "application/ld+json",
            br#"{"@id": "urn:a", "urn:p": {"@id": "urn:b"}}"#,
        )
        .unwrap();
        assert_eq!(jsonld.len(), 1);
        let blank = parse("application/ld+json", br#"{"urn:p": "x"}"#).unwrap();
        assert_eq!(blank_nodes(&blank).len(), 1, "an @id-less node is blank");
    }
}
