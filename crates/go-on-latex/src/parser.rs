//! Parser for LaTeX math expressions.
//!
//! Converts a LaTeX math string into an AST of `MathNode` values.
//! Supports: letters, numbers, `^` (superscript), `_` (subscript),
//! `\frac{a}{b}`, `\sqrt{x}`, `\sum`, Greek letters, and basic operators.

use crate::MathNode;

/// Token types for the LaTeX lexer.
#[derive(Debug, Clone, PartialEq)]
enum Token {
    /// A literal character (letter, digit, punctuation, operator)
    Char(char),
    /// A LaTeX command like \alpha, \frac, \sum
    Command(String),
    /// Opening brace
    BeginGroup,
    /// Closing brace
    EndGroup,
    /// Superscript
    Superscript,
    /// Subscript
    Subscript,
    /// End of input
    Eof,
}

/// Lexer: converts a LaTeX string into a stream of tokens.
struct Lexer {
    chars: Vec<char>,
    pos: usize,
}

impl Lexer {
    fn new(input: &str) -> Self {
        Self {
            chars: input.chars().collect(),
            pos: 0,
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<char> {
        let c = self.chars.get(self.pos).copied();
        if c.is_some() {
            self.pos += 1;
        }
        c
    }

    fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek() {
            if c.is_ascii_whitespace() {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Token {
        self.skip_whitespace();
        match self.peek() {
            None => Token::Eof,
            Some('{') => {
                self.advance();
                Token::BeginGroup
            }
            Some('}') => {
                self.advance();
                Token::EndGroup
            }
            Some('^') => {
                self.advance();
                Token::Superscript
            }
            Some('_') => {
                self.advance();
                Token::Subscript
            }
            Some('\\') => {
                self.advance(); // consume backslash
                let mut name = String::new();
                while let Some(c) = self.peek() {
                    if c.is_ascii_alphabetic() {
                        name.push(c);
                        self.advance();
                    } else {
                        break;
                    }
                }
                Token::Command(name)
            }
            Some(c) => {
                self.advance();
                Token::Char(c)
            }
        }
    }
}

/// Recursive descent parser for LaTeX math expressions.
struct Parser {
    lexer: Lexer,
    current: Token,
}

impl Parser {
    fn new(input: &str) -> Self {
        let mut lexer = Lexer::new(input);
        let current = lexer.next_token();
        Self { lexer, current }
    }

    fn advance(&mut self) {
        self.current = self.lexer.next_token();
    }

    /// Parse a complete LaTeX math expression until end-of-input or `}`.
    fn parse_expression(&mut self) -> Vec<MathNode> {
        let mut nodes = Vec::new();
        loop {
            match &self.current {
                Token::Eof | Token::EndGroup => break,
                _ => {
                    let node = self.parse_term();
                    nodes.push(node);
                }
            }
        }
        nodes
    }

    /// Parse a single term, handling superscript/subscript after it.
    fn parse_term(&mut self) -> MathNode {
        let base = self.parse_atom();

        // Check for superscript/subscript following
        let mut sup: Option<Box<MathNode>> = None;
        let mut sub: Option<Box<MathNode>> = None;

        loop {
            match &self.current {
                Token::Superscript => {
                    self.advance();
                    sup = Some(Box::new(self.parse_atom()));
                }
                Token::Subscript => {
                    self.advance();
                    sub = Some(Box::new(self.parse_atom()));
                }
                _ => break,
            }
        }

        if sup.is_some() || sub.is_some() {
            MathNode::SubSup {
                base: Box::new(base),
                sup,
                sub,
            }
        } else {
            base
        }
    }

    /// Parse an atomic element: a group `{...}`, a command, or a single character.
    fn parse_atom(&mut self) -> MathNode {
        match &self.current {
            Token::BeginGroup => {
                self.advance();
                let nodes = self.parse_expression();
                if self.current == Token::EndGroup {
                    self.advance();
                }
                if nodes.len() == 1 {
                    nodes.into_iter().next().unwrap()
                } else {
                    MathNode::Group(nodes)
                }
            }
            Token::Command(name) => {
                let name = name.clone();
                self.advance();
                self.parse_command(&name)
            }
            Token::Char(c) => {
                let c = *c;
                self.advance();
                MathNode::Text(c.to_string())
            }
            _ => {
                // Skip unexpected tokens
                self.advance();
                MathNode::Text(String::new())
            }
        }
    }

    /// Parse a LaTeX command and return the corresponding node.
    fn parse_command(&mut self, name: &str) -> MathNode {
        match name {
            "frac" => {
                let num = self
                    .parse_optional_group()
                    .unwrap_or_else(|| MathNode::Text(String::new()));
                let den = self
                    .parse_optional_group()
                    .unwrap_or_else(|| MathNode::Text(String::new()));
                MathNode::Fraction(Box::new(num), Box::new(den))
            }
            "sqrt" => {
                // Optional [n] for root degree
                let degree = if self.current == Token::Char('[') {
                    self.advance();
                    let mut s = String::new();
                    while let Token::Char(c) = &self.current {
                        s.push(*c);
                        self.advance();
                    }
                    if self.current == Token::Char(']') {
                        self.advance();
                    }
                    Some(s)
                } else {
                    None
                };
                let radicand = self
                    .parse_optional_group()
                    .unwrap_or_else(|| MathNode::Text(String::new()));
                if let Some(deg) = degree {
                    MathNode::Sqrt(Box::new(MathNode::Group(vec![
                        MathNode::Text(deg),
                        MathNode::Text(",".to_string()),
                        radicand,
                    ])))
                } else {
                    MathNode::Sqrt(Box::new(radicand))
                }
            }
            "sum" | "sum_" => {
                let sub = if self.current == Token::Subscript {
                    self.advance();
                    Some(Box::new(self.parse_atom()))
                } else {
                    None
                };
                let sup = if self.current == Token::Superscript {
                    self.advance();
                    Some(Box::new(self.parse_atom()))
                } else {
                    None
                };
                MathNode::Sum(Box::new(MathNode::Text(String::new())), sup, sub)
            }
            "alpha" | "beta" | "gamma" | "delta" | "epsilon" | "zeta" | "eta" | "theta"
            | "iota" | "kappa" | "lambda" | "mu" | "nu" | "xi" | "omicron" | "pi" | "rho"
            | "sigma" | "tau" | "upsilon" | "phi" | "chi" | "psi" | "omega" => {
                MathNode::GreekChar(name.to_string())
            }
            "Alpha" | "Beta" | "Gamma" | "Delta" | "Epsilon" | "Zeta" | "Eta" | "Theta"
            | "Iota" | "Kappa" | "Lambda" | "Mu" | "Nu" | "Xi" | "Omicron" | "Pi" | "Rho"
            | "Sigma" | "Tau" | "Upsilon" | "Phi" | "Chi" | "Psi" | "Omega" => {
                MathNode::GreekChar(name.to_string())
            }
            "times" => MathNode::Operator("\u{00D7}".to_string()),
            "div" => MathNode::Operator("\u{00F7}".to_string()),
            "pm" | "plusmn" => MathNode::Operator("\u{00B1}".to_string()),
            "cdot" => MathNode::Operator("\u{00B7}".to_string()),
            "to" | "rightarrow" => MathNode::Operator("\u{2192}".to_string()),
            "leftarrow" => MathNode::Operator("\u{2190}".to_string()),
            "infty" | "infinity" => MathNode::Operator("\u{221E}".to_string()),
            "ne" | "neq" => MathNode::Operator("\u{2260}".to_string()),
            "le" | "leq" => MathNode::Operator("\u{2264}".to_string()),
            "ge" | "geq" => MathNode::Operator("\u{2265}".to_string()),
            "approx" => MathNode::Operator("\u{2248}".to_string()),
            "Rightarrow" => MathNode::Operator("\u{21D2}".to_string()),
            "Leftarrow" | "lArr" => MathNode::Operator("\u{21D0}".to_string()),
            "int" | "integral" => MathNode::Operator("\u{222B}".to_string()),
            "partial" => MathNode::Operator("\u{2202}".to_string()),
            "forall" => MathNode::Operator("\u{2200}".to_string()),
            "exists" => MathNode::Operator("\u{2203}".to_string()),
            "nabla" => MathNode::Operator("\u{2207}".to_string()),
            "in" => MathNode::Operator("\u{2208}".to_string()),
            "notin" => MathNode::Operator("\u{2209}".to_string()),
            "subset" => MathNode::Operator("\u{2282}".to_string()),
            "supset" => MathNode::Operator("\u{2283}".to_string()),
            "subseteq" | "subeq" => MathNode::Operator("\u{2286}".to_string()),
            "supseteq" | "supeq" => MathNode::Operator("\u{2287}".to_string()),
            "cup" | "union" => MathNode::Operator("\u{222A}".to_string()),
            "cap" | "intersection" => MathNode::Operator("\u{2229}".to_string()),
            "sin" | "cos" | "tan" | "log" | "ln" | "lim" | "det" | "max" | "min" => {
                MathNode::Operator(name.to_string())
            }
            "left" | "right" | "big" | "Big" | "bigg" | "Bigg" => {
                // Consume the next character (the bracket type)
                let next = self.parse_atom();
                MathNode::Group(vec![MathNode::Text(" ".to_string()), next])
            }
            "quad" | "qquad" | "," | ":" | ";" | "!" => MathNode::Text(" ".to_string()),
            "text" => {
                if self.current == Token::BeginGroup {
                    self.advance();
                    let mut text = String::new();
                    while let Token::Char(c) = &self.current {
                        text.push(*c);
                        self.advance();
                    }
                    if self.current == Token::EndGroup {
                        self.advance();
                    }
                    MathNode::Text(text)
                } else {
                    MathNode::Text(String::new())
                }
            }
            _ => {
                // Unknown command - just return the name as text
                MathNode::Text(format!("\\{}", name))
            }
        }
    }

    /// Parse an optional group `{...}` if present, otherwise return None.
    fn parse_optional_group(&mut self) -> Option<MathNode> {
        if self.current == Token::BeginGroup {
            self.advance();
            let nodes = self.parse_expression();
            if self.current == Token::EndGroup {
                self.advance();
            }
            Some(if nodes.len() == 1 {
                nodes.into_iter().next().unwrap()
            } else {
                MathNode::Group(nodes)
            })
        } else {
            // If no group, parse a single atom
            Some(self.parse_atom())
        }
    }
}

/// Parse a LaTeX math expression string into a Vec<MathNode>.
///
/// # Example
/// ```
/// use go_on_latex::parser::parse;
///
/// let nodes = parse("x^2 + \\frac{1}{2}");
/// assert!(!nodes.is_empty());
/// ```
pub fn parse(input: &str) -> Vec<MathNode> {
    if input.trim().is_empty() {
        return vec![MathNode::Text(String::new())];
    }
    let mut parser = Parser::new(input);
    parser.parse_expression()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_text() {
        let nodes = parse("x + y = z");
        assert!(nodes.len() >= 5);
    }

    #[test]
    fn test_superscript() {
        let nodes = parse("x^2");
        assert!(matches!(nodes[0], MathNode::SubSup { .. }));
    }

    #[test]
    fn test_subscript() {
        let nodes = parse("x_i");
        assert!(matches!(nodes[0], MathNode::SubSup { .. }));
    }

    #[test]
    fn test_fraction() {
        let nodes = parse("\\frac{1}{2}");
        assert!(matches!(nodes[0], MathNode::Fraction(..)));
    }

    #[test]
    fn test_sqrt() {
        let nodes = parse("\\sqrt{x}");
        assert!(matches!(nodes[0], MathNode::Sqrt(..)));
    }

    #[test]
    fn test_greek() {
        let nodes = parse("\\alpha \\beta \\pi");
        assert!(nodes.len() >= 3);
    }

    #[test]
    fn test_sum() {
        let nodes = parse("\\sum_{i=0}^{n}");
        assert!(matches!(nodes[0], MathNode::Sum(..)));
    }
}
