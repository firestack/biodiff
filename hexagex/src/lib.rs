pub use regex::bytes as regex_bytes;
use regex_syntax::ast::{self, Ast, Span};
use std::fmt::Write;
use std::ops::Deref;

pub fn hexagex(hexagex: &str) -> Result<regex_bytes::Regex, Error> {
    || -> Result<regex_bytes::Regex, InternalError> {
        let ast = ast::parse::ParserBuilder::new()
            .ignore_whitespace(false)
            .build()
            .parse(hexagex)?;
        let real_ast = binary_ast_to_final_ast(&ast)?;
        drop(ast);
        let mut ret = String::new();
        ast::print::Printer::new()
            .print(&real_ast, &mut ret)
            .unwrap();
        let regex = regex_bytes::RegexBuilder::new(&ret)
            .ignore_whitespace(false)
            .case_insensitive(false)
            .unicode(false)
            .multi_line(true)
            .octal(true)
            .dot_matches_new_line(true)
            .build()?;
        Ok(regex)
    }()
    .map_err(|x| Error::new(x, hexagex))
}

fn binary_ast_to_final_ast(ast: &Ast) -> Result<Ast, InternalError> {
    match binary_ast_to_maybe_ast(ast)? {
        Either::Left(l) => l.try_into(),
        Either::Right(r) => Ok(r),
    }
}

fn binary_ast_to_maybe_ast(ast: &Ast) -> Result<Either<PartialSequence, Ast>, InternalError> {
    match ast {
        Ast::Literal(lit) if is_text_escape_literal(lit) => {
            Err(InternalError::IncompleteEscape(lit.span))
        }
        Ast::Literal(lit) => Ok(Either::Left(PartialElement::try_from(lit)?.sequence())),
        Ast::Dot(span) => Ok(Either::Left(
            PartialElement::full(4, Some(*span)).sequence(),
        )),
        Ast::Assertion(a) => match a.kind {
            ast::AssertionKind::StartLine => Ok(Either::Right(Ast::Assertion(ast::Assertion {
                kind: ast::AssertionKind::StartText,
                ..a.clone()
            }))),
            ast::AssertionKind::EndLine => Ok(Either::Right(Ast::Assertion(ast::Assertion {
                kind: ast::AssertionKind::EndText,
                ..a.clone()
            }))),
            _ => Err(InternalError::InvalidCharacter(a.span)),
        },
        Ast::Class(c) => Ok(Either::Left(PartialElement::try_from(c)?.sequence())),
        Ast::Repetition(l) => Ok(Either::Right(Ast::Repetition(ast::Repetition {
            ast: Box::new(binary_ast_to_final_ast(&l.ast)?),
            span: l.span,
            op: l.op.clone(),
            greedy: l.greedy,
        }))),
        Ast::Group(g) => Ok(Either::Right(Ast::Group(ast::Group {
            ast: Box::new(binary_ast_to_final_ast(&g.ast)?),
            span: g.span,
            kind: g.kind.clone(),
        }))),
        Ast::Alternation(alt) => {
            let asts = alt
                .asts
                .iter()
                .map(binary_ast_to_final_ast)
                .collect::<Result<_, _>>()?;
            Ok(Either::Right(Ast::Alternation(ast::Alternation {
                span: alt.span,
                asts,
            })))
        }
        Ast::Concat(concat) => PartialSequence::try_from(concat).map(Either::Left),
        Ast::Empty(span) => Ok(Either::Right(Ast::Empty(*span))),
        Ast::Flags(flags) => Ok(Either::Right(Ast::Flags(flags.clone()))),
    }
}

impl TryFrom<&ast::Concat> for PartialSequence {
    type Error = InternalError;

    fn try_from(value: &ast::Concat) -> Result<Self, Self::Error> {
        let mut elements = Vec::new();
        let mut escape_span = None;
        for ast in value.asts.iter() {
            if escape_span.is_some() {
                escape_span = None;
                elements.push(Either::Right(ast.clone()));
                continue;
            }
            match ast {
                Ast::Concat(v) => elements.append(&mut PartialSequence::try_from(v)?.elements),
                Ast::Literal(lit) if is_text_escape_literal(lit) => escape_span = Some(lit.span),
                _ => match binary_ast_to_maybe_ast(ast)? {
                    Either::Left(mut l) => elements.append(&mut l.elements),
                    Either::Right(r) => elements.push(Either::Right(r)),
                },
            }
        }
        if let Some(span) = escape_span {
            Err(InternalError::IncompleteEscape(span))
        } else {
            Ok(PartialSequence {
                span: value.span,
                elements,
            })
        }
    }
}

impl TryFrom<&ast::Class> for PartialElement {
    type Error = InternalError;

    fn try_from(value: &ast::Class) -> Result<Self, Self::Error> {
        match value {
            ast::Class::Unicode(c) => Err(InternalError::Unicode(c.span)),
            ast::Class::Perl(c) => Ok(c.into()),
            ast::Class::Bracketed(c) => c.try_into(),
        }
    }
}

impl From<&ast::ClassPerl> for PartialElement {
    fn from(pc: &ast::ClassPerl) -> Self {
        let span = pc.span;
        let mut ret = match pc.kind {
            ast::ClassPerlKind::Digit => PartialElement::from_range(&[b'0'..=b'9'], span, 8),
            ast::ClassPerlKind::Space => PartialElement {
                length: 8,
                span: Some(span),
                values: vec![b'\t', b'\n', 0x0c, b'\r', b' '],
            },
            ast::ClassPerlKind::Word => PartialElement::from_range(
                &[b'0'..=b'9', b'a'..=b'z', b'A'..=b'Z', b'_'..=b'_'],
                span,
                8,
            ),
        };
        if pc.negated {
            ret = !&ret;
        }
        ret
    }
}

impl TryFrom<&ast::ClassBracketed> for PartialElement {
    type Error = InternalError;

    fn try_from(value: &ast::ClassBracketed) -> Result<Self, Self::Error> {
        let mut ret: PartialElement = match &value.kind {
            ast::ClassSet::Item(i) => i.try_into()?,
            ast::ClassSet::BinaryOp(i) => i.try_into()?,
        };
        if value.negated {
            ret = !&ret;
        }
        Ok(ret.with_span(value.span))
    }
}

impl TryFrom<&ast::ClassSet> for PartialElement {
    type Error = InternalError;

    fn try_from(value: &ast::ClassSet) -> Result<Self, Self::Error> {
        match value {
            ast::ClassSet::Item(i) => i.try_into(),
            ast::ClassSet::BinaryOp(i) => i.try_into(),
        }
    }
}

impl TryFrom<&ast::ClassSetBinaryOp> for PartialElement {
    type Error = InternalError;

    fn try_from(value: &ast::ClassSetBinaryOp) -> Result<Self, Self::Error> {
        let lhs_conv = PartialElement::try_from(&*value.lhs)?;
        let rhs_conv = PartialElement::try_from(&*value.rhs)?;
        if lhs_conv.length != rhs_conv.length {
            return Err(InternalError::LengthMismatch(LengthMismatch {
                lengths: (lhs_conv.length, rhs_conv.length),
                spans: (lhs_conv.span.unwrap(), rhs_conv.span.unwrap()),
            }));
        }
        let result = match value.kind {
            ast::ClassSetBinaryOpKind::Intersection => &lhs_conv & &rhs_conv,
            ast::ClassSetBinaryOpKind::Difference => &lhs_conv - &rhs_conv,
            ast::ClassSetBinaryOpKind::SymmetricDifference => &lhs_conv ^ &rhs_conv,
        };
        Ok(result.with_span(value.span))
    }
}

impl TryFrom<&ast::ClassSetItem> for PartialElement {
    type Error = InternalError;

    fn try_from(value: &ast::ClassSetItem) -> Result<Self, Self::Error> {
        match value {
            ast::ClassSetItem::Empty(span) => Ok(PartialElement::empty(Some(*span))),
            ast::ClassSetItem::Literal(lit) if is_text_escape_literal(lit) => {
                Err(InternalError::InvalidEscapePosition(lit.span))
            }
            ast::ClassSetItem::Literal(lit) => lit.try_into(),
            ast::ClassSetItem::Range(range) => range.try_into(),
            ast::ClassSetItem::Ascii(ascii) => Ok(ascii.into()),
            ast::ClassSetItem::Unicode(_) => Err(InternalError::Unicode(*value.span())),
            ast::ClassSetItem::Perl(perl) => Ok(perl.into()),
            ast::ClassSetItem::Bracketed(bracketed) => bracketed.deref().try_into(),
            ast::ClassSetItem::Union(union) => union.try_into(),
        }
    }
}

fn is_text_escape_literal(lit: &ast::Literal) -> bool {
    matches!(
        lit.kind,
        ast::LiteralKind::Special(ast::SpecialLiteralKind::Tab)
    )
}

impl TryFrom<&ast::Literal> for PartialElement {
    type Error = InternalError;

    fn try_from(value: &ast::Literal) -> Result<Self, Self::Error> {
        let (val, len) = match value.kind {
            ast::LiteralKind::Verbatim if [' ', '\t', '\n'].contains(&value.c) => {
                return Ok(PartialElement::empty(Some(value.span)))
            }
            ast::LiteralKind::Verbatim
            | ast::LiteralKind::Punctuation
            | ast::LiteralKind::Special(_) => {
                if !value.c.is_ascii() {
                    return Err(InternalError::Unicode(value.span));
                }
                match value.c {
                    'I' | 'i' => (1, 1),
                    'O' | 'o' => (0, 1),
                    '_' => {
                        return Ok(PartialElement::full(1, Some(value.span)));
                    }
                    otherwise => {
                        let x = u8::from_str_radix(&otherwise.to_string(), 16)
                            .map_err(|_| InternalError::InvalidCharacter(value.span))?;
                        (x, 4)
                    }
                }
            }
            ast::LiteralKind::Octal
            | ast::LiteralKind::HexFixed(ast::HexLiteralKind::X)
            | ast::LiteralKind::HexBrace(_) => {
                let v = value.c as u32;
                match u8::try_from(v) {
                    Ok(x) => (x, 8),
                    Err(_) => return Err(InternalError::Unicode(value.span)),
                }
            }
            ast::LiteralKind::HexFixed(_) => return Err(InternalError::Unicode(value.span)),
        };
        Ok(PartialElement {
            length: len,
            span: Some(value.span),
            values: vec![val],
        })
    }
}

impl TryFrom<&ast::ClassSetRange> for PartialElement {
    type Error = InternalError;

    fn try_from(value: &ast::ClassSetRange) -> Result<Self, Self::Error> {
        if let Some(lit) = [&value.start, &value.end]
            .iter()
            .find(|x| is_text_escape_literal(x))
        {
            return Err(InternalError::InvalidEscapePosition(lit.span));
        }
        let start = PartialElement::try_from(&value.start)?;
        let end = PartialElement::try_from(&value.end)?;
        if start.length != end.length {
            return Err(InternalError::LengthMismatch(LengthMismatch {
                lengths: (start.length, end.length),
                spans: (start.span.unwrap(), end.span.unwrap()),
            }));
        }
        let start_val = start
            .values
            .first()
            .unwrap()
            .min(end.values.first().unwrap());
        let end_val = start.values.last().unwrap().max(end.values.last().unwrap());
        Ok(PartialElement::from_range(
            &[*start_val..=*end_val],
            value.span,
            start.length,
        ))
    }
}

impl From<&ast::ClassAscii> for PartialElement {
    fn from(value: &ast::ClassAscii) -> Self {
        let ranges = match value.kind {
            ast::ClassAsciiKind::Alnum => [b'0'..=b'9', b'A'..=b'Z', b'a'..=b'z'].as_ref(),
            ast::ClassAsciiKind::Alpha => &[b'A'..=b'Z', b'a'..=b'z'],
            ast::ClassAsciiKind::Ascii => &[0x00..=0x7f],
            ast::ClassAsciiKind::Blank => &[b' '..=b' ', b'\t'..=b'\t'],
            ast::ClassAsciiKind::Cntrl => &[0x00..=0x1f, 0x7f..=0x7f],
            ast::ClassAsciiKind::Digit => &[b'0'..=b'9'],
            ast::ClassAsciiKind::Graph => &[b'!'..=b'~'],
            ast::ClassAsciiKind::Lower => &[b'a'..=b'z'],
            ast::ClassAsciiKind::Print => &[b' '..=b'~'],
            ast::ClassAsciiKind::Punct => &[b'!'..=b'/', b':'..=b'@', b'['..=b'`', b'{'..=b'~'],
            ast::ClassAsciiKind::Space => &[b'\t'..=b'\r', b' '..=b' '],
            ast::ClassAsciiKind::Upper => &[b'A'..=b'Z'],
            ast::ClassAsciiKind::Word => &[b'0'..=b'9', b'A'..=b'Z', b'a'..=b'z', b'_'..=b'_'],
            ast::ClassAsciiKind::Xdigit => &[b'0'..=b'9', b'A'..=b'F', b'a'..=b'f'],
        };
        let mut ret = PartialElement::from_range(ranges, value.span, 8);
        if value.negated {
            ret = !&ret;
        }
        ret
    }
}

impl TryFrom<&ast::ClassSetUnion> for PartialElement {
    type Error = InternalError;

    fn try_from(value: &ast::ClassSetUnion) -> Result<Self, Self::Error> {
        let mut result: PartialElement = match value.items.get(0) {
            Some(x) => x.try_into()?,
            None => return Ok(PartialElement::empty(Some(value.span))),
        };
        for item in &value.items[1..] {
            let conv: PartialElement = item.try_into()?;
            if conv.length != result.length {
                return Err(InternalError::LengthMismatch(LengthMismatch {
                    lengths: (result.length, conv.length),
                    spans: (result.span.unwrap(), conv.span.unwrap()),
                }));
            }
            result = &result | &conv;
        }
        Ok(result)
    }
}

impl TryFrom<PartialSequence> for Ast {
    type Error = InternalError;

    fn try_from(value: PartialSequence) -> Result<Self, Self::Error> {
        if value.elements.is_empty() {
            return Ok(Ast::Empty(value.span));
        }
        let mut ast = ast::Concat {
            span: value.span,
            asts: Vec::new(),
        };
        let mut current_byte_values = PartialElement::empty(None);
        for element in value.elements {
            let element = match element {
                Either::Left(partial) => partial,
                Either::Right(astpart) => {
                    if current_byte_values.length > 0 {
                        ast.asts.push(current_byte_values.try_into()?);
                        current_byte_values = PartialElement::empty(Some(Span::new(
                            astpart.span().start,
                            astpart.span().end,
                        )));
                    }
                    ast.asts.push(astpart);
                    continue;
                }
            };
            let (new_byte_values, next_byte_values) = element.split(8 - current_byte_values.length);
            current_byte_values = current_byte_values.concat(&new_byte_values);
            if let Some(byte_values) = next_byte_values {
                ast.asts.push(current_byte_values.try_into()?);
                current_byte_values = byte_values;
            }
        }
        if current_byte_values.length > 0 {
            ast.asts.push(current_byte_values.try_into()?);
        }
        Ok(Ast::Concat(ast))
    }
}

#[derive(Debug)]
struct PartialSequence {
    span: Span,
    elements: Vec<Either<PartialElement, Ast>>,
}

#[derive(Debug)]
struct PartialElement {
    /// up to 8
    length: u8,
    span: Option<Span>,
    values: Vec<u8>,
}

fn add_byte_range_to_class(class: &mut ast::ClassSetUnion, range: (u8, u8)) {
    let byte_literal = |x: u8| ast::Literal {
        span: class.span,
        kind: ast::LiteralKind::HexFixed(ast::HexLiteralKind::X),
        c: char::from_u32(x as u32).unwrap(),
    };
    let new = if range.0 == range.1 {
        ast::ClassSetItem::Literal(byte_literal(range.0))
    } else {
        ast::ClassSetItem::Range(ast::ClassSetRange {
            span: class.span,
            start: byte_literal(range.0),
            end: byte_literal(range.1),
        })
    };
    class.items.push(new);
}

impl TryFrom<PartialElement> for Ast {
    type Error = InternalError;

    fn try_from(value: PartialElement) -> Result<Self, Self::Error> {
        let span = value
            .span
            .expect("Called try_from for PartialElement to Ast without span");
        if value.length == 0 {
            return Ok(Ast::Empty(span));
        } else if value.length != 8 {
            return Err(InternalError::AlignmentError(AlignmentError {
                is: value.length % 8,
                required: 8,
                span,
            }));
        }
        if let [v] = value.values.as_slice() {
            return Ok(Ast::Literal(ast::Literal {
                span,
                kind: ast::LiteralKind::HexFixed(ast::HexLiteralKind::X),
                c: *v as char,
            }));
        }
        let mut byte_class = ast::ClassSetUnion {
            span,
            items: Vec::new(),
        };
        let mut current_range = match value.values.get(0) {
            None => return Ok(Ast::Empty(span)),
            Some(a) => (*a, *a),
        };
        for &i in &value.values[1..] {
            if i - 1 == current_range.1 {
                current_range = (current_range.0, i);
            } else {
                add_byte_range_to_class(&mut byte_class, current_range);
                current_range = (i, i);
            }
        }
        add_byte_range_to_class(&mut byte_class, current_range);
        Ok(Ast::Class(ast::Class::Bracketed(ast::ClassBracketed {
            span,
            negated: false,
            kind: ast::ClassSet::Item(ast::ClassSetItem::Union(byte_class)),
        })))
    }
}

impl std::ops::Not for &PartialElement {
    type Output = PartialElement;

    fn not(self) -> Self::Output {
        let max = self.max_possible();
        let mut values = Vec::new();
        let mut value_index = 0;
        for value in 0..=max {
            match self.values.get(value_index) {
                Some(&x) if x == value => value_index += 1,
                _ => values.push(value),
            }
        }
        PartialElement {
            length: self.length,
            span: self.span,
            values,
        }
    }
}

impl std::ops::BitOr for &PartialElement {
    type Output = PartialElement;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.map_pair(rhs, |a, b| a.or(b))
    }
}

impl std::ops::BitAnd for &PartialElement {
    type Output = PartialElement;

    fn bitand(self, rhs: Self) -> Self::Output {
        self.map_pair(rhs, |a, b| a.and(b))
    }
}

impl std::ops::BitXor for &PartialElement {
    type Output = PartialElement;

    fn bitxor(self, rhs: Self) -> Self::Output {
        self.map_pair(rhs, |a, b| match (a, b) {
            (None, None) | (Some(_), Some(_)) => None,
            (Some(a), None) | (None, Some(a)) => Some(a),
        })
    }
}

impl std::ops::Sub for &PartialElement {
    type Output = PartialElement;

    fn sub(self, rhs: Self) -> Self::Output {
        self.map_pair(rhs, |a, b| {
            a.and_then(|x| if b.is_some() { None } else { Some(x) })
        })
    }
}

impl PartialElement {
    fn from_range(ranges: &[std::ops::RangeInclusive<u8>], span: Span, length: u8) -> Self {
        let mut values = Vec::new();
        values.extend(ranges.iter().flat_map(|x| x.clone()));
        Self {
            length,
            span: Some(span),
            values,
        }
    }
    fn split(self, byte_bound_offset: u8) -> (Self, Option<Self>) {
        if byte_bound_offset >= self.length {
            return (self, None);
        }
        let first = self.extract_high(byte_bound_offset);
        let next = self.extract_low(byte_bound_offset);
        (first, Some(next))
    }
    fn extract_low(&self, split_point: u8) -> Self {
        let new_len = self.length - split_point;
        let mask = self.max_possible();
        let mut ret = Vec::new();
        if new_len > 0 {
            for value in self.values.iter().map(|x| x & mask) {
                if ret.last() != Some(&value) {
                    ret.push(value)
                }
            }
        } else {
            ret = vec![0];
        }
        PartialElement {
            length: new_len,
            span: self.span,
            values: ret,
        }
    }
    fn extract_high(&self, split_point: u8) -> Self {
        let mut ret = Vec::new();
        if split_point != self.length {
            for value in self
                .values
                .iter()
                .map(|x| ((*x as u16) >> (self.length - split_point)) as u8)
            {
                if ret.last() != Some(&value) {
                    ret.push(value)
                }
            }
        } else {
            ret = self.values.clone();
        }
        PartialElement {
            length: split_point,
            span: self.span,
            values: ret,
        }
    }
    fn concat(&self, rhs: &Self) -> Self {
        PartialElement {
            values: self
                .values
                .iter()
                .flat_map(|x| {
                    rhs.values
                        .iter()
                        .map(move |y| ((*x as u16) << rhs.length) as u8 | y)
                })
                .collect(),
            length: self.length + rhs.length,
            span: combine_spans(self.span, rhs.span),
        }
    }
    fn map_pair<F: FnMut(Option<u8>, Option<u8>) -> Option<u8>>(
        &self,
        other: &Self,
        mut f: F,
    ) -> Self {
        let mut self_value = self.values.iter().map(|x| std::cmp::Reverse(*x)).peekable();
        let mut other_value = other
            .values
            .iter()
            .map(|x| std::cmp::Reverse(*x))
            .peekable();
        let mut ret = Vec::new();
        loop {
            let (self_next, other_next) = (self_value.peek(), other_value.peek());
            if self_next.is_none() && other_next.is_none() {
                break;
            }
            if let Some(a) = match self_next.cmp(&other_next).reverse() {
                std::cmp::Ordering::Less => f(self_value.next().map(|x| x.0), None),
                std::cmp::Ordering::Equal => f(
                    self_value.next().map(|x| x.0),
                    other_value.next().map(|x| x.0),
                ),
                std::cmp::Ordering::Greater => f(None, other_value.next().map(|x| x.0)),
            } {
                ret.push(a)
            }
        }
        PartialElement {
            length: self.length,
            span: self.span,
            values: ret,
        }
    }
    fn empty(span: Option<Span>) -> Self {
        Self {
            length: 0,
            span,
            values: vec![0],
        }
    }
    fn full(len: u8, span: Option<Span>) -> Self {
        let mut r = Self {
            length: len,
            span,
            values: vec![0],
        };
        r.values = (0..=r.max_possible()).collect();
        r
    }
    fn sequence(self) -> PartialSequence {
        PartialSequence {
            span: self.span.unwrap(),
            elements: vec![Either::Left(self)],
        }
    }
    fn max_possible(&self) -> u8 {
        ((1u16 << self.length) as u8).wrapping_sub(1)
    }
    fn with_span(mut self, span: Span) -> Self {
        self.span = Some(span);
        self
    }
}

fn combine_spans(lhs: Option<Span>, rhs: Option<Span>) -> Option<Span> {
    match (lhs, rhs) {
        (None, None) => None,
        (None, x) | (x, None) => x,
        (Some(l), Some(r)) => Some(Span::new(l.start.min(r.start), l.end.max(r.end))),
    }
}

#[derive(Debug)]
pub enum Error {
    RegexError(String),
    HexagexError(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Error::RegexError(s) | Error::HexagexError(s) => s,
        };
        write!(f, "{}", s)
    }
}

impl std::error::Error for Error {}

fn write_with_span(regex: &str, span: Span) -> String {
    let mut out = String::new();
    let lines = regex.lines().collect::<Vec<_>>();
    let line_number = lines.len();
    for (i, line) in lines.into_iter().enumerate() {
        let _ = writeln!(&mut out, "{}", line);
        let i = i + 1;
        if span.start.line <= i && span.end.line >= i {
            let start = if span.start.line == i {
                span.start.column - 1
            } else {
                0
            };
            let span_len = if span.end.line == i {
                span.end.column - 1
            } else {
                line.len()
            } - start;
            out.extend(
                std::iter::repeat(' ')
                    .take(start)
                    .chain(std::iter::repeat('^').take(span_len)),
            );
            if i != line_number {
                let _ = writeln!(&mut out);
            }
        }
    }
    out
}

impl Error {
    fn new(err: InternalError, regex: &str) -> Self {
        let mut ret = String::new();
        match err {
            InternalError::RegexError(e) => Error::RegexError(e),
            InternalError::AlignmentError(a) => {
                let _ = writeln!(
                    &mut ret,
                    "hexagex error: mismatch in alignment, should be {}, is {}",
                    a.required, a.is
                );
                ret += &write_with_span(regex, a.span);
                Error::HexagexError(ret)
            }
            InternalError::LengthMismatch(l) => {
                let _ = writeln!(
                    &mut ret,
                    "hexagex error: length mismatch; this part has a bit length of {}:",
                    l.lengths.0
                );
                ret += &write_with_span(regex, l.spans.0);
                let _ = writeln!(
                    &mut ret,
                    "\nwhile this part has a bit length of {}:",
                    l.lengths.1
                );
                ret += &write_with_span(regex, l.spans.1);
                Error::HexagexError(ret)
            }
            InternalError::Unicode(span) => {
                let _ = writeln!(
                    &mut ret,
                    "hexagex error: unicode is not supported; offending part:"
                );
                ret += &write_with_span(regex, span);
                Error::HexagexError(ret)
            }
            InternalError::InvalidCharacter(span) => {
                let _ = writeln!(&mut ret, "hexagex error: invalid character here:");
                ret += &write_with_span(regex, span);
                let _ = write!(&mut ret, "\nNote: use 0-9a-f and . (wildcard) for hexadecimal parts with 4 bits lengths\nI and O and _ (wildcard) for bits and \\x00-\\xff for bytes");
                Error::HexagexError(ret)
            }
            InternalError::IncompleteEscape(span) => {
                let _ = writeln!(&mut ret, "hexagex error: incomplete escape:");
                ret += &write_with_span(regex, span);
                Error::HexagexError(ret)
            }
            InternalError::InvalidEscapePosition(span) => {
                let _ = writeln!(
                    &mut ret,
                    "hexagex error: escape characters cannot be placed here:"
                );
                ret += &write_with_span(regex, span);
                let _ = write!(&mut ret, "\nNote: if you want to have text characters in braces, place the \\t before it");
                Error::HexagexError(ret)
            }
        }
    }
}

#[derive(Debug)]
enum InternalError {
    RegexError(String),
    AlignmentError(AlignmentError),
    LengthMismatch(LengthMismatch),
    Unicode(Span),
    InvalidCharacter(Span),
    IncompleteEscape(Span),
    InvalidEscapePosition(Span),
}

impl From<regex_syntax::ast::Error> for InternalError {
    fn from(e: regex_syntax::ast::Error) -> Self {
        InternalError::RegexError(e.to_string())
    }
}

impl From<regex::Error> for InternalError {
    fn from(e: regex::Error) -> Self {
        InternalError::RegexError(e.to_string())
    }
}

#[derive(Debug)]
struct AlignmentError {
    is: u8,
    required: u8,
    span: Span,
}

#[derive(Debug)]
struct LengthMismatch {
    lengths: (u8, u8),
    spans: (Span, Span),
}

#[derive(Debug)]
enum Either<Left, Right> {
    Left(Left),
    Right(Right),
}

#[cfg(test)]
mod tests {
    use super::hexagex;
    fn h(s: &(impl AsRef<[u8]> + ?Sized)) -> Vec<u8> {
        hex::decode(s).unwrap()
    }
    fn matches<'a>(expr: &str, content: &'a [u8]) -> Vec<regex::bytes::Match<'a>> {
        hexagex(expr)
            .unwrap()
            .find_iter(&content)
            .collect::<Vec<_>>()
    }
    fn get_underline_ranges(underline: &str) -> Vec<std::ops::Range<usize>> {
        let mut current_range = None;
        let mut ranges = Vec::new();
        for (i, c) in underline.chars().enumerate() {
            match (c, current_range) {
                ('^', None) => {
                    if i % 2 != 0 {
                        panic!("Underline must begin at even index")
                    }
                    current_range = Some(i / 2)
                }
                ('$', Some(start)) => {
                    if i % 2 != 1 {
                        panic!("Underline must end in odd index")
                    }
                    ranges.push(start..1 + i / 2);
                    current_range = None;
                }
                ('-', Some(_)) | (' ', None) => (),
                ('^', Some(_)) => panic!("New underline started without finishing previous one"),
                ('$', None) => panic!("Underline finished without starting one"),
                ('-', None) => panic!("Underline continued without starting one"),
                (' ', Some(_)) => panic!("Underline ended without $"),
                (x, _) => panic!("Unknown underline character '{}'", x),
            }
        }
        ranges
    }
    fn test_matches(regex: &str, hexcontent: &str, underlined: &str) {
        //println!("{}", hexagex(regex).unwrap().to_string());
        let content = h(hexcontent);
        let mut matches = matches(regex, &content).into_iter();
        for range in get_underline_ranges(underlined).into_iter() {
            let m = matches
                .next()
                .expect(&format!("Missing match at {:?}", range));
            assert_eq!(
                m.range(),
                range,
                "Wrong range, should be {:?}, is {:?}",
                range,
                m.range()
            );
            assert_eq!(
                m.as_bytes(),
                &content[range.clone()],
                "Wrong match content, should be {:?}, is {:?}",
                &content[range],
                m.as_bytes()
            )
        }
    }
    #[test]
    #[rustfmt::skip]
    fn simple_string() {
        test_matches(
            "0123456789abcdef",
            "4130123456789abcdef00123456789abcdef012300",
            "                    ^--------------$      ",
        )
    }
    #[test]
    #[rustfmt::skip]
    fn match_newline() {
        test_matches(
            "002000",
            "200020000000200020002000",
            "  ^----$  ^----$  ^----$"
        )
    }
    #[test]
    #[rustfmt::skip]
    fn match_start() {
        test_matches(
            "^0010",
            "0010001020001020202020000010",
            "^--$                        "
        )
    }
    #[test]
    #[rustfmt::skip]
    fn match_end() {
        test_matches(
            "01(..)*23$",
            "110456789a874534230123200123",
            "                  ^--------$",
        )
    }
    #[test]
    fn match_class() {
        test_matches(
            "[0-e][1-F]",
            "0001f0f1ee1100011011202230345061ff",
            "  ^$    ^$^$  ^$  ^$  ^$  ^$  ^$  ",
        )
    }
    #[test]
    fn match_bits() {
        test_matches(
            "IO[IO]I[1-a] (f[IO]I[^O]I)+ ..",
            "91f7fffe00b5f79afe99fff7fff7ef910000",
            "^------$  ^----$  ^----------$      ",
        )
    }
    #[test]
    fn match_choice() {
        test_matches(
            "(IOIOa IOIOIOIO|5[0-f--0-4--6-f]){2}",
            "a55aaaaa5555aaaa5a5a56aaaa55aaaaaaaa5555",
            "    ^----$^----$      ^----$^------$^--$",
        )
    }
    #[test]
    fn match_byte_range() {
        test_matches(
            r#"\x00 [\x45-\xf3] [^\xee]"#,
            "0045ed0044ed00f3de00f40061ee",
            "^----$      ^----$          ",
        )
    }
    #[test]
    fn match_classes() {
        test_matches(
            r#"[[:alpha:]\x00] [[:^digit:]\x31] \w \D"#,
            "4a31616300615f3a313161612031ff",
            "^------$^------$      ^------$",
        )
    }
    #[test]
    fn match_text_escape() {
        test_matches(
            r"\t(Hello World)",
            "48656c6c6f204a6f686e2e2e2e206e6f20776169742c2048656c6c6f20576f726c64210a",
            "                                              ^--------------------$    ",
        )
    }
}
