"""Inspect the requested minimal program before executing its logic in isolation."""

import ast
import contextlib
import io
import pathlib
import sys

source = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
tree = ast.parse(source, filename="hello.py")

# The requested program has no imports, global computation, or external effects.
# Accept optional docstrings and a -> None return annotation.
def without_docstring(body):
    if body and isinstance(body[0], ast.Expr):
        value = body[0].value
        if isinstance(value, ast.Constant) and isinstance(value.value, str):
            return body[1:]
    return body


tree.body = without_docstring(tree.body)
assert len(tree.body) == 2, "expected main() and a main guard only"
function = tree.body[0]
assert isinstance(function, ast.FunctionDef), "expected main()"
assert function.returns is None or (
    isinstance(function.returns, ast.Constant) and function.returns.value is None
), "unsupported return annotation"
function.returns = None
function.body = without_docstring(function.body)
expected = ast.parse('def main():\n    print("Hello, world!")\n\nif __name__ == "__main__":\n    main()\n')
assert ast.dump(tree) == ast.dump(expected), "unexpected Hello World logic"

# A valid import has no output; repeated main() calls behave consistently.
namespace = {"__name__": "hello"}
stdout, stderr = io.StringIO(), io.StringIO()
with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
    exec(compile(tree, "hello.py", "exec"), namespace)
assert stdout.getvalue() == stderr.getvalue() == "", "import must have no output"
with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
    assert namespace["main"]() is None
    assert namespace["main"]() is None
assert stdout.getvalue() == "Hello, world!\nHello, world!\n"
assert stderr.getvalue() == ""
print("logic verified")
