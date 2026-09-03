"""Prototype parser for .run — implements GRAMMAR.ebnf. Reports every deviation."""
import re, sys, os

KEYWORDS = {"let","if","else","for","match","case","default","run","end","exec","in"}
SOURCES  = {"ARG","ENV","FLAG","RUN"}

class Err(Exception):
    def __init__(self, line, msg): self.line, self.msg = line, msg

# ---------- expression tokenizer (operates on one logical expression string) ----------
TOK = re.compile(r"""
    (?P<ws>\s+)
  | (?P<num>\d+(?:\.\d+)?)
  | (?P<op>==|!=|<=|>=|&&|\|\||[-+*/%<>!?\[\]().,])
  | (?P<id>[A-Za-z_][A-Za-z0-9_-]*)
""", re.X)

def skip_interp(s, i, lineno):
    """s[i:] starts at '{{'. Return index just past the matching '}}'.
    Nested {{ }} are opaque: quotes inside them do not affect the outer string."""
    depth, i = 0, i
    while i < len(s):
        if s.startswith("{{", i): depth += 1; i += 2
        elif s.startswith("}}", i):
            depth -= 1; i += 2
            if depth == 0: return i
        else: i += 1
    raise Err(lineno, "unterminated {{ ... }} interpolation")

def scan_string(s, i, lineno, raw):
    """Hand-rolled because a regex cannot see that quotes inside {{ }} are opaque."""
    start = i
    i += 2 if raw else 1                      # past r" or "
    while i < len(s):
        if s.startswith("{{", i): i = skip_interp(s, i, lineno); continue
        c = s[i]
        if c == "\\": i += 2; continue      # in raw strings the backslash is kept in
                                             # the value, but \" still does not terminate
        if c == '"': return s[start:i+1], i + 1
        i += 1
    raise Err(lineno, "unterminated string literal")

def lex_expr(s, lineno):
    out, i = [], 0
    while i < len(s):
        if s.startswith('r"', i):
            tok, i = scan_string(s, i, lineno, True);  out.append(("raw", tok)); continue
        if s[i] == '"':
            tok, i = scan_string(s, i, lineno, False); out.append(("str", tok)); continue
        m = TOK.match(s, i)
        if not m: raise Err(lineno, f"unexpected character {s[i]!r} in expression")
        i = m.end()
        if m.lastgroup != "ws": out.append((m.lastgroup, m.group()))
    return out

class P:
    def __init__(self, toks, lineno): self.t, self.i, self.ln = toks, 0, lineno
    def peek(self): return self.t[self.i] if self.i < len(self.t) else (None, None)
    def take(self):
        if self.i >= len(self.t): raise Err(self.ln, "unexpected end of expression")
        self.i += 1; return self.t[self.i-1]
    def eat(self, val):
        k, v = self.peek()
        if v == val: self.i += 1; return True
        return False
    def expect(self, val):
        if not self.eat(val): raise Err(self.ln, f"expected {val!r}, got {self.peek()[1]!r}")

    def expr(self):  # chain
        n = self.or_()
        while self.eat("?"): n = ("chain", n, self.or_())
        return n
    def or_(self):
        n = self.and_()
        while self.eat("||"): n = ("or", n, self.and_())
        return n
    def and_(self):
        n = self.cmp()
        while self.eat("&&"): n = ("and", n, self.cmp())
        return n
    def cmp(self):
        n = self.add()
        while self.peek()[1] in ("==","!=","<","<=",">",">="):
            op = self.take()[1]; n = (op, n, self.add())
        return n
    def add(self):
        n = self.mul()
        while self.peek()[1] in ("+","-"): op = self.take()[1]; n = (op, n, self.mul())
        return n
    def mul(self):
        n = self.unary()
        while self.peek()[1] in ("*","/","%"): op = self.take()[1]; n = (op, n, self.unary())
        return n
    def unary(self):
        if self.peek()[1] in ("!","-"): op = self.take()[1]; return (op, self.unary())
        return self.postfix()
    def postfix(self):
        n = self.primary()
        while True:
            if self.eat("("):
                args = []
                if self.peek()[1] != ")":
                    args.append(self.expr())
                    while self.eat(","):
                        if self.peek()[1] == ")": break
                        args.append(self.expr())
                self.expect(")"); n = ("call", n, args)
            elif self.eat("["):
                idx = self.expr(); self.expect("]"); n = ("index", n, idx)
            else: return n
    def primary(self):
        k, v = self.peek()
        if k == "num": self.take(); return ("num", v)
        if k in ("str","raw"): self.take(); return (k, v)
        if v == "[":
            self.take(); items = []
            if self.peek()[1] != "]":
                items.append(self.expr())
                while self.eat(","):
                    if self.peek()[1] == "]": break
                    items.append(self.expr())
            self.expect("]"); return ("list", items)
        if v == "(":
            self.take(); n = self.expr(); self.expect(")"); return n
        if k == "id":
            self.take()
            if v in SOURCES and self.peek()[1] == ".":
                self.take(); _, name = self.take(); return ("source", v, name)
            if v in ("true","false"): return ("bool", v)
            return ("id", v)
        raise Err(self.ln, f"unexpected token {v!r} in expression")

def parse_expr(s, lineno):
    p = P(lex_expr(s, lineno), lineno)
    n = p.expr()
    if p.i != len(p.t): raise Err(lineno, f"trailing tokens after expression: {p.t[p.i:][:3]}")
    return n

# ---------- line-level parser ----------
def parse_file(path):
    src = open(path).read().split("\n")
    i, out, depth = 0, [], 0
    while i < len(src):
        raw = src[i]; lineno = i + 1; s = raw.strip()
        if not s or s.startswith("#"): i += 1; continue

        if s.startswith("$ ") or s == "$":
            # backslash continuation
            while s.endswith("\\") and i + 1 < len(src): i += 1; s = src[i].strip()
            out.append(("shell", lineno)); i += 1; continue

        if s.startswith("exec "):
            indent = raw[:len(raw) - len(raw.lstrip())]
            j = i + 1
            while j < len(src):
                if src[j] == indent + "end" or (indent == "" and src[j].rstrip() == "end"): break
                j += 1
            if j >= len(src):
                raise Err(lineno, "exec block never closed by an 'end' at its own indentation")
            out.append(("exec", lineno)); i = j + 1; continue

        if s.startswith("."):
            m = re.match(r"^\.([A-Za-z][\w-]*(?:\.[A-Za-z_][\w-]*)*)\s*(?:=\s*(.+))?$", s)
            if not m: raise Err(lineno, f"malformed property: {s!r}")
            if m.group(2): parse_expr(m.group(2), lineno)
            out.append(("prop", lineno)); i += 1; continue

        # multi-line constructs: join while brackets are unbalanced
        joined, j = s, i
        while joined.count("[") > joined.count("]") and j + 1 < len(src):
            j += 1; joined += " " + src[j].strip()
        consumed = j - i + 1

        head = joined.split(None, 1)[0]
        rest = joined.split(None, 1)[1] if " " in joined else ""

        if head == "let":
            m = re.match(r"^let\s+([A-Za-z_][\w-]*)\s*=\s*(.+)$", joined)
            if not m: raise Err(lineno, f"malformed let: {joined[:60]!r}")
            parse_expr(m.group(2), lineno); out.append(("let", lineno))
        elif head == "if":     parse_expr(rest, lineno); out.append(("if", lineno)); depth += 1
        elif head == "for":
            m = re.match(r"^for\s+([A-Za-z_][\w-]*)\s+in\s+(.+)$", joined)
            if not m: raise Err(lineno, f"malformed for: {joined[:60]!r}")
            parse_expr(m.group(2), lineno); out.append(("for", lineno)); depth += 1
        elif head == "match":  parse_expr(rest, lineno); out.append(("match", lineno)); depth += 1
        elif head == "case":   out.append(("case", lineno))
        elif head == "default":out.append(("default", lineno))
        elif head == "else":   out.append(("else", lineno))
        elif head == "end":    depth -= 1; out.append(("end", lineno))
        elif head == "run":    out.append(("run", lineno))
        else:
            # bare call statement, or an assignment
            if re.match(r"^[A-Za-z_][\w-]*\s*=\s*", joined):
                parse_expr(joined.split("=",1)[1], lineno); out.append(("assign", lineno))
            else:
                parse_expr(joined, lineno); out.append(("call", lineno))
        i += consumed
    if depth != 0: raise Err(len(src), f"unbalanced blocks (depth {depth})")
    return out

if __name__ == "__main__":
    files = []
    for proj in sys.argv[1:]:
        for root, dirs, fs in os.walk(proj):
            dirs[:] = [d for d in dirs if d not in ("node_modules",".git")]
            files += [os.path.join(root,f) for f in fs if f.endswith(".run")]
    ok, fails = 0, []
    for f in sorted(files):
        try: parse_file(f); ok += 1
        except Err as e: fails.append((f, e.line, e.msg))
        except Exception as e: fails.append((f, "?", f"{type(e).__name__}: {e}"))
    print(f"parsed {ok}/{len(files)} files")
    for f, l, m in fails: print(f"  FAIL {f}:{l}\n       {m}")
