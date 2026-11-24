use nom::{
    error::{ErrorKind, ParseError},
    Compare, CompareResult, Err, FindSubstring, IResult, Needed, Offset,
};
use std::{
    ops::Deref,
    slice::SliceIndex,
    str::{CharIndices, Chars},
};

use super::{
    combinator::{token, GreenElement},
    SyntaxKind,
};
use crate::config::ParseConfig;

/// A custom Input struct
///
/// It helps us to pass the `ParseConfig` all the way down to each parsers
#[derive(Clone, Copy, Debug)]
pub struct Input<'a> {
    pub(crate) s: &'a str,
    pub(crate) c: &'a ParseConfig,
}

impl<'a> Input<'a> {
    #[inline]
    pub(crate) fn of(&self, i: &'a str) -> Input<'a> {
        Input { s: i, c: self.c }
    }

    /// A part of the input (Blossom patch: nom 8 dropped `Slice`).
    #[inline]
    pub(crate) fn slice<R: SliceIndex<str, Output = str>>(&self, range: R) -> Input<'a> {
        self.of(&self.s[range])
    }

    /// `(rest, first count bytes)`, as nom's `take_split`.
    #[inline]
    pub(crate) fn take_split(&self, count: usize) -> (Input<'a>, Input<'a>) {
        let (l, r) = self.s.split_at(count);
        (self.of(r), self.of(l))
    }

    #[inline]
    pub fn as_str(&self) -> &'a str {
        self.s
    }

    #[inline]
    pub fn token(&self, kind: SyntaxKind) -> GreenElement {
        token(kind, self.s)
    }

    #[inline]
    pub fn text_token(&self) -> GreenElement {
        token(SyntaxKind::TEXT, self.s)
    }

    #[inline]
    pub fn ws_token(&self) -> GreenElement {
        token(SyntaxKind::WHITESPACE, self.s)
    }

    #[inline]
    pub fn nl_token(&self) -> GreenElement {
        token(SyntaxKind::NEW_LINE, self.s)
    }
}

impl<'a> Deref for Input<'a> {
    type Target = str;

    #[inline]
    fn deref(&self) -> &'a str {
        self.s
    }
}

impl<'a> From<(&'a str, &'a ParseConfig)> for Input<'a> {
    fn from(value: (&'a str, &'a ParseConfig)) -> Self {
        Input {
            s: value.0,
            c: value.1,
        }
    }
}

impl<'a, 'b> FindSubstring<&'b str> for Input<'a> {
    fn find_substring(&self, substr: &str) -> Option<usize> {
        self.s.find(substr)
    }
}

impl<'a, 'b> Compare<&'b str> for Input<'a> {
    #[inline]
    fn compare(&self, t: &'b str) -> CompareResult {
        self.s.compare(t)
    }

    #[inline]
    fn compare_no_case(&self, t: &'b str) -> CompareResult {
        self.s.compare_no_case(t)
    }
}

/// Maps an error at a `&str` position back onto the input.
fn lift<'a, E: ParseError<Input<'a>>>(
    of: &Input<'a>,
    r: IResult<&'a str, &'a str, (&'a str, ErrorKind)>,
) -> IResult<Input<'a>, Input<'a>, E> {
    match r {
        Ok((l, r)) => Ok((of.of(l), of.of(r))),
        Err(Err::Error((i, kind))) => Err(Err::Error(E::from_error_kind(of.of(i), kind))),
        Err(Err::Failure((i, kind))) => Err(Err::Failure(E::from_error_kind(of.of(i), kind))),
        Err(Err::Incomplete(x)) => Err(Err::Incomplete(x)),
    }
}

// Blossom patch: nom 8 merged `InputLength`, `InputIter`, `InputTake`,
// `InputTakeAtPosition` and `Slice` into one `Input` trait.
impl<'a> nom::Input for Input<'a> {
    type Item = char;
    type Iter = Chars<'a>;
    type IterIndices = CharIndices<'a>;

    #[inline]
    fn input_len(&self) -> usize {
        self.s.len()
    }
    #[inline]
    fn take(&self, index: usize) -> Self {
        self.of(&self.s[..index])
    }
    #[inline]
    fn take_from(&self, index: usize) -> Self {
        self.of(&self.s[index..])
    }
    #[inline]
    fn take_split(&self, index: usize) -> (Self, Self) {
        Input::take_split(self, index)
    }
    #[inline]
    fn position<P>(&self, predicate: P) -> Option<usize>
    where
        P: Fn(Self::Item) -> bool,
    {
        self.s.find(predicate)
    }
    #[inline]
    fn iter_elements(&self) -> Self::Iter {
        self.s.chars()
    }
    #[inline]
    fn iter_indices(&self) -> Self::IterIndices {
        self.s.char_indices()
    }
    #[inline]
    fn slice_index(&self, count: usize) -> Result<usize, Needed> {
        nom::Input::slice_index(&self.s, count)
    }
    #[inline]
    fn split_at_position<P, E: ParseError<Self>>(&self, predicate: P) -> IResult<Self, Self, E>
    where
        P: Fn(Self::Item) -> bool,
    {
        lift(self, self.s.split_at_position(predicate))
    }
    #[inline]
    fn split_at_position1<P, E: ParseError<Self>>(
        &self,
        predicate: P,
        e: ErrorKind,
    ) -> IResult<Self, Self, E>
    where
        P: Fn(Self::Item) -> bool,
    {
        lift(self, self.s.split_at_position1(predicate, e))
    }
    #[inline]
    fn split_at_position_complete<P, E: ParseError<Self>>(
        &self,
        predicate: P,
    ) -> IResult<Self, Self, E>
    where
        P: Fn(Self::Item) -> bool,
    {
        lift(self, self.s.split_at_position_complete(predicate))
    }
    #[inline]
    fn split_at_position1_complete<P, E: ParseError<Self>>(
        &self,
        predicate: P,
        e: ErrorKind,
    ) -> IResult<Self, Self, E>
    where
        P: Fn(Self::Item) -> bool,
    {
        lift(self, self.s.split_at_position1_complete(predicate, e))
    }
}

impl<'a> Offset for Input<'a> {
    fn offset(&self, second: &Self) -> usize {
        self.s.offset(second.s)
    }
}
