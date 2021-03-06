use nom::{
    branch::alt,
    bytes::complete::{escaped, tag, take_while},
    character::complete::{none_of, not_line_ending, one_of, space0},
    combinator::{map, opt},
    error::VerboseError,
    multi::separated_list0,
    number::complete::double as read_double,
    sequence::{delimited, preceded, separated_pair, terminated, tuple},
    // Err::{Error as NomError, Failure as NomFailure, Incomplete as NomIncomplete},
    IResult,
};

use crate::parser::comment::Comment;
use crate::parser::label::LabelList;
use crate::parser::metric_data::SampleData;

type NomRes<I, O> = IResult<I, O, VerboseError<I>>;

// https://prometheus.io/docs/concepts/data_model/#metric-names-and-labels
fn is_metric_char(s: char) -> bool {
    s.is_alphanumeric() || s == '_' || s == ':' || s == '.'
}

fn read_quoted_string(input: &str) -> NomRes<&str, String> {
    let normal = none_of("\\\"");
    let escapable = one_of("\"\\'n");
    let escape_non_empty = escaped(normal, '\\', escapable);
    let reduce_special_chars = |s: &str| s.replace("\\\\", "\\");
    delimited(
        tag("\""),
        map(alt((escape_non_empty, tag(""))), reduce_special_chars),
        tag("\""),
    )(input)
}

fn read_variable_name(input: &str) -> NomRes<&str, &str> {
    preceded(space0, take_while(is_metric_char))(input)
}

fn read_label(input: &str) -> NomRes<&str, LabelList> {
    opt(delimited(
        preceded(space0, tag("{")),
        separated_list0(
            preceded(space0, terminated(tag(","), space0)),
            separated_pair(
                read_variable_name,
                preceded(space0, terminated(tag("="), space0)),
                read_quoted_string,
            ),
        ),
        preceded(space0, tag("}")),
    ))(input)
    .map(|(out, label)| (out, label.unwrap_or_default().into()))
}

fn read_value(input: &str) -> NomRes<&str, f64> {
    preceded(
        space0,
        alt((
            map(tag("+Inf"), |_| f64::INFINITY),
            map(tag("-Inf"), |_| f64::NEG_INFINITY),
            read_double,
        )),
    )(input)
}

fn read_timestamp(input: &str) -> NomRes<&str, Option<i64>> {
    let read_timestamp_as_i64 = map(read_double, |f: f64| f as i64);
    opt(preceded(space0, read_timestamp_as_i64))(input)
}

fn read_comment_line(input: &str) -> NomRes<&str, Comment> {
    let comment_identifier = tuple((tag("#"), space0));
    let known_comment_types = alt((tag("HELP"), tag("TYPE")));

    tuple((
        preceded(comment_identifier, known_comment_types),
        preceded(space0, read_variable_name),
        preceded(space0, not_line_ending),
    ))(input)
    .map(|(out, (c_type, metric, desc))| {
        (out, Comment::new(metric.into(), c_type.into(), desc.into()))
    })
}

// https://prometheus.io/docs/instrumenting/exposition_formats/#comments-help-text-and-type-information
fn read_sample_line(input: &str) -> NomRes<&str, SampleData> {
    tuple((read_variable_name, read_label, read_value, read_timestamp))(input).map(
        |(out, (name, label, value, timestamp))| {
            (out, SampleData::new(name.into(), label, value, timestamp))
        },
    )
}
/// Tries to parse a `&str` line as a sample and returns [`SampleData`]
/// containg the metric name, labels and value if it succeeds.
/// # Examples:
/// ```
/// use prometheus_wire::parser::{SampleData, LabelList, try_read_sample};
/// use std::collections::HashMap;
///
/// let line = r#"http_requests_total{method="post",code="200"} 1.5e3 1395066363000"#;
/// let opt_metric = try_read_sample(line);
///
/// let mut map = HashMap::new();
/// map.insert(String::from("method"), String::from("post"));
/// map.insert(String::from("code"), String::from("200"));
///
/// assert_eq!(
///     opt_metric,
///     Some(SampleData::new(
///         String::from("http_requests_total"),
///         LabelList::from_map(map),
///         1500.0,
///         Some(1395066363000))
///     ));
///
/// let metric = opt_metric.unwrap();
///
/// assert_eq!(metric.labels.get_string("method"), Some(&String::from("post")));
/// assert_eq!(metric.labels.get_number("code"), Some(200.0));
///
/// assert_eq!(try_read_sample("# test"), None);
/// ```
pub fn try_read_sample(line: &str) -> Option<SampleData> {
    read_sample_line(line).ok().map(|(_, metric)| metric)
}

/// Tries to parse a `&str` line as a comment and returns [`Comment`] if it succeeds.
///
/// # Examples:
/// ```
/// use prometheus_wire::parser::{Comment, CommentType, try_read_comment};
/// assert_eq!(
///     try_read_comment("# HELP test1 this is a test"),
///     Some(Comment::new(String::from("test1"), CommentType::HELP, String::from("this is a test")))
/// );
///
/// assert_eq!(try_read_comment("metric 12345"), None);
/// ```
pub fn try_read_comment(line: &str) -> Option<Comment> {
    read_comment_line(line).ok().map(|(_, comment)| comment)
}

#[cfg(test)]
mod tests {
    use crate::parser::comment::CommentType;
    use crate::parser::line_parser::*;
    use std::collections::HashMap;

    #[test]
    fn test_read_variable_name() {
        assert_eq!(read_variable_name("alfa_123").unwrap(), ("", "alfa_123"));
        assert_eq!(read_variable_name(" beta:456 ").unwrap(), (" ", "beta:456"));
        assert_eq!(read_variable_name(" gama.789{").unwrap(), ("{", "gama.789"));
    }

    #[test]
    fn test_read_quoted_string() {
        read_quoted_string("").unwrap_err();
        assert_eq!(read_quoted_string("\"\"").unwrap(), ("", "".into()));
        assert_eq!(
            read_quoted_string("\" alfa_123 \"").unwrap(),
            ("", " alfa_123 ".into())
        );
        assert_eq!(
            read_quoted_string("\"new\\nline\"").unwrap(),
            ("", "new\\nline".into())
        );
        assert_eq!(
            read_quoted_string("\" C:\\\\test\\\\ \"").unwrap(),
            ("", " C:\\test\\ ".into())
        );
        assert_eq!(
            read_quoted_string("\"beta:\\\"456\\\"\"").unwrap(),
            ("", "beta:\\\"456\\\"".into())
        );
    }

    #[test]
    fn test_read_label() {
        assert_eq!(read_label("").unwrap(), ("", LabelList::new()));
        assert_eq!(read_label("{}").unwrap(), ("", LabelList::new()));
        assert_eq!(read_label(" ").unwrap(), (" ", LabelList::new()));
        assert_eq!(read_label(" {} ").unwrap(), (" ", LabelList::new()));

        let mut h1 = HashMap::new();
        h1.insert("alfa".into(), "1".into());

        assert_eq!(
            read_label("{alfa=\"1\"}").unwrap(),
            ("", LabelList::from_map(h1.clone()))
        );
        assert_eq!(
            read_label("{ alfa = \"1\" }").unwrap(),
            ("", LabelList::from_map(h1.clone()))
        );

        let mut h2 = HashMap::new();
        h2.insert("a_b:1".into(), "test\\\"1\\\"".into());
        h2.insert("543_a.76".into(), "C:\\test\\".into());

        let s = " { a_b:1 = \"test\\\"1\\\"\" , 543_a.76=\"C:\\\\test\\\\\"}";

        assert_eq!(
            read_label(s).unwrap(),
            ("", LabelList::from_map(h2.clone()))
        );

        let s_no_spaces = s.replace(" ", "");
        assert_eq!(
            read_label(s_no_spaces.as_str()).unwrap(),
            ("", LabelList::from_map(h2.clone()))
        );

        // doesn't work (yet) and should be tested again if some case is found
        // assert_eq!(read_label("{ alfa = \"1\", }").unwrap(), ("", Label::fromMap(h1)));
    }

    #[test]
    fn test_read_value() {
        read_value("").unwrap_err();
        read_value(" ").unwrap_err();
        assert_eq!(read_value(" +154.0").unwrap(), ("", 154.0));
        assert_eq!(read_value("-1500.0 ").unwrap(), (" ", -1500.0));
        assert_eq!(read_value("1.5e-03 5").unwrap(), (" 5", 0.0015));
        assert_eq!(read_value("+Inf ").unwrap(), (" ", f64::INFINITY));
        assert_eq!(read_value("-1.7560473e+07").unwrap(), ("", -17560473.0));
        assert_eq!(
            read_value(" -Inf  1234").unwrap(),
            ("  1234", f64::NEG_INFINITY)
        );
    }

    #[test]
    fn test_read_timestamp() {
        assert_eq!(read_timestamp("").unwrap(), ("", None));
        assert_eq!(read_timestamp(" 1").unwrap(), ("", Some(1)));
        assert_eq!(read_timestamp("    ").unwrap(), ("    ", None));
        assert_eq!(read_timestamp("123456789").unwrap(), ("", Some(123456789)));
        assert_eq!(
            read_timestamp("-987654321 5").unwrap(),
            (" 5", Some(-987654321))
        );
    }

    #[test]
    fn test_read_comment_line() {
        read_comment_line("# alfa").unwrap_err();
        assert_eq!(
            read_comment_line("# HELP").unwrap(),
            ("", Comment::new("".into(), CommentType::HELP, "".into()))
        );
        assert_eq!(
            read_comment_line("# HELP node_cpu_seconds_total Seconds the CPUs spent in each mode.")
                .unwrap(),
            (
                "",
                Comment::new(
                    "node_cpu_seconds_total".into(),
                    CommentType::HELP,
                    "Seconds the CPUs spent in each mode.".into()
                )
            )
        );
        assert_eq!(
            read_comment_line("#    TYPE     node_cpu_seconds_total counter").unwrap(),
            (
                "",
                Comment::new(
                    "node_cpu_seconds_total".into(),
                    CommentType::TYPE,
                    "counter".into()
                )
            )
        );
        assert_eq!(
            read_comment_line("#    HELP     alfa").unwrap(),
            (
                "",
                Comment::new("alfa".into(), CommentType::HELP, "".into())
            )
        );
    }

    #[test]
    fn test_read_metric_line() {
        let s = "something_weird{problem=\"division by zero\"} +Inf -3982045";

        let mut h1 = HashMap::new();
        h1.insert("problem".into(), "division by zero".into());
        let l = LabelList::from_map(h1);

        assert_eq!(
            read_sample_line(s).unwrap(),
            (
                "",
                SampleData::new(
                    String::from("something_weird"),
                    l,
                    f64::INFINITY,
                    Some(-3982045)
                )
            )
        );

        let s = "msdos_file_access_time_seconds{path=\"C:\\\\DIR\\\\FILE.TXT\",error=\"Cannot find file:\\n\\\"FILE.TXT\\\"\"} 1.458255915e9";

        let mut h1 = HashMap::new();
        h1.insert("path".into(), "C:\\DIR\\FILE.TXT".into());
        h1.insert(
            "error".into(),
            "Cannot find file:\\n\\\"FILE.TXT\\\"".into(),
        );
        let l = LabelList::from_map(h1);
        assert_eq!(
            read_sample_line(s).unwrap(),
            (
                "",
                SampleData::new(
                    String::from("msdos_file_access_time_seconds"),
                    l,
                    1458255915.0,
                    None
                )
            )
        );
    }
}
