"""Deterministic faulty commit corpus and independent acceptance oracle."""

from release.files import atomic_write_text, create_directory

import hashlib
import json
import math
import os
from release.process import run as run_process
import sys
from pathlib import Path

from e2e.scripts.harness.checks import require

FAMILIES = [
    (
        "factorial",
        "n!; 0! is 1",
        "result = 1\nfor k in range(1, n):\n    result *= k\nreturn result",
    ),
    (
        "fibonacci",
        "nth Fibonacci number, F(0)=0, F(1)=1",
        "a, b = 0, 1\nfor _ in range(n + 1):\n    a, b = b, a + b\nreturn a",
    ),
    (
        "triangular",
        "sum of integers from 1 through n inclusive; 0 for n=0",
        "return n * (n - 1) // 2",
    ),
    (
        "square_sum",
        "sum of k*k for integers k from 1 through n inclusive; 0 for n=0",
        "return sum(k * k for k in range(1, n))",
    ),
    ("power_two", "2 raised to the nth power; 1 for n=0", "return 2 ** (n + 1)"),
    (
        "count_digits",
        "number of decimal digits in n; zero has one digit",
        "count = 0\nwhile n:\n    count += 1\n    n //= 10\nreturn count",
    ),
    (
        "is_prime",
        "bool: whether n is prime; 0 and 1 are not prime",
        "if n < 2:\n    return False\nfor k in range(2, math.isqrt(n)):\n    if n % k == 0:\n        return False\nreturn True",
    ),
    (
        "double_factorial",
        "n!! = n*(n-2)*...; 0!! and 1!! are 1",
        "result = 1\nfor k in range(n - 1, 0, -2):\n    result *= k\nreturn result",
    ),
    ("ceil_half", "smallest integer greater than or equal to n/2", "return n // 2"),
    (
        "popcount",
        "number of 1 bits in n's binary representation; zero has zero",
        "return bin(n)[2:].count('0')",
    ),
]


def git(repo, *args, env=None):
    return run_process(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
        env=env,
        timeout=60,
    ).stdout.strip()


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def expected(family, n):
    if family == "factorial":
        return math.factorial(n)
    if family == "fibonacci":
        a, b = 0, 1
        for _ in range(n):
            a, b = b, a + b
        return a
    if family == "triangular":
        return sum(range(n + 1))
    if family == "square_sum":
        return n * (n + 1) * (2 * n + 1) // 6
    if family == "power_two":
        return 1 << n
    if family == "count_digits":
        return len(str(n))
    if family == "is_prime":
        return n >= 2 and all(n % k for k in range(2, math.isqrt(n) + 1))
    if family == "double_factorial":
        return math.prod(range(n, 0, -2))
    if family == "ceil_half":
        return (n + 1) // 2
    if family == "popcount":
        return n.bit_count()
    raise ValueError(family)


def check(path, family):
    # Run generated/model-written code in a bounded subprocess. Expected results
    # live here, outside the model's editable fake repository.
    values = list(range(101)) + [127, 128, 199, 200, 999, 1000]
    if family in ("factorial", "fibonacci", "double_factorial"):
        values = [n for n in values if n <= 200]
    cases = [(n, expected(family, n)) for n in values]
    before_hash = digest(path)
    source = """import hashlib,json,pathlib,sys
path=pathlib.Path(sys.argv[1]);source=path.read_bytes();ns={"__name__":"candidate","__file__":str(path)}
exec(compile(source,str(path),"exec"),ns)
f=ns["calculate"];observations=[]
inputs=json.loads(sys.argv[2])+[-1,-2,-100,True,False,1.0,1.5,"5",None,[],{},complex(1,0)]
for n in inputs:
    try:
        got=f(n)
        observations.append({"type":type(got).__name__,"repr":repr(got)})
    except Exception as error:
        kind="TypeError" if isinstance(error,TypeError) else "ValueError" if isinstance(error,ValueError) else type(error).__name__
        observations.append({"exception":kind})
print(json.dumps({"source_sha256":hashlib.sha256(source).hexdigest(),"observations":observations}))
"""
    result = run_process(
        [sys.executable, "-B", "-c", source, str(path), json.dumps(values)],
        capture_output=True,
        text=True,
        timeout=20,
        env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"),
        check=False,
    )
    marker = None
    if result.returncode == 0:
        try:
            marker = json.loads(result.stdout.splitlines()[-1])
        except (ValueError, IndexError):
            marker = None
    wanted = [{"type": type(want).__name__, "repr": repr(want)} for _, want in cases]
    wanted += [{"exception": "ValueError"}] * 3 + [{"exception": "TypeError"}] * 9
    passed = (
        result.returncode == 0
        and digest(path) == before_hash
        and marker == {"source_sha256": before_hash, "observations": wanted}
    )
    return {
        "passed": passed,
        "cases": len(wanted),
        "diagnostic": (result.stdout + result.stderr)[-4000:],
    }


def create(output, count=100):
    output = Path(output).resolve()
    repo = output / "repo"
    create_directory(repo, mode=0o700)
    git(repo, "init", "--quiet")
    git(repo, "config", "user.name", "Hive scenario")
    git(repo, "config", "user.email", "scenario@example.invalid")
    create_directory(repo / "functions", mode=0o700)
    create_directory(repo / "contracts", mode=0o700)
    assignments = []
    baseline = []
    for i in range(1, count + 1):
        key = f"{i:03}"
        family, description, faulty = FAMILIES[(i - 1) % len(FAMILIES)]
        module = f"functions/function_{key}.py"
        contract_path = f"contracts/function_{key}.json"
        source = (
            "import math\n\ndef calculate(n):\n"
            "    if isinstance(n, bool) or not isinstance(n, int):\n"
            '        raise TypeError("expected an integer, excluding bool")\n'
            '    if n < 0:\n        raise ValueError("expected a nonnegative integer")\n'
            + "".join("    " + line + "\n" for line in faulty.splitlines())
        )
        atomic_write_text(repo / module, source)
        contract = {
            "function": "calculate(n)",
            "family": family,
            "result": description,
            "input": "Python int excluding bool; non-integers including bool raise TypeError; negative integers raise ValueError",
        }
        atomic_write_text(repo / contract_path, json.dumps(contract, indent=2) + "\n")
        git(repo, "add", module, contract_path)
        timestamp = f"{1600000000 + i} +0000"
        env = dict(os.environ, GIT_AUTHOR_DATE=timestamp, GIT_COMMITTER_DATE=timestamp)
        git(
            repo,
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            f"Implement {family} ({key})",
            env=env,
        )
        commit = git(repo, "rev-parse", "HEAD")
        review = output / "reviews" / key
        create_directory(review.parent, exist_ok=True, mode=0o700)
        git(repo, "worktree", "add", "--quiet", "--detach", str(review), commit)
        original = check(review / module, family)
        if original["passed"]:
            raise RuntimeError(f"Fixture {key} was not faulty")
        baseline.append({"id": key, **original})
        assignments.append(
            {
                "id": key,
                "commit": commit,
                "module": module,
                "contract_path": contract_path,
                "contract": contract,
                "original_sha256": digest(review / module),
                "contract_sha256": digest(review / contract_path),
                "review_cwd": str(review),
                "working_file": str(repo / module),
            }
        )
    manifest = {
        "repo": str(repo),
        "head": git(repo, "rev-parse", "HEAD"),
        "commits": git(repo, "rev-list", "--reverse", "HEAD").splitlines(),
        "all_commits": sorted(git(repo, "rev-list", "--all").splitlines()),
        "refs": git(repo, "for-each-ref", "--format=%(refname) %(objectname)"),
        "assignments": assignments,
    }
    atomic_write_text(output / "baseline.json", json.dumps(baseline, indent=2) + "\n")
    manifest["baseline_sha256"] = digest(output / "baseline.json")
    atomic_write_text(
        output / "manifest.json", json.dumps(manifest, indent=2) + "\n", create_once=True
    )
    return manifest


def load_manifest(output):
    """The last-published manifest admits the exact completed baseline."""
    manifest = json.loads((output / "manifest.json").read_text())
    require(
        manifest.get("baseline_sha256") == digest(output / "baseline.json"),
        "Incomplete or changed fixture baseline",
    )
    return manifest


def verify_repo(manifest):
    repo = Path(manifest["repo"])
    assignments = manifest["assignments"]
    require(git(repo, "rev-parse", "HEAD") == manifest["head"], "HEAD changed")
    require(
        git(repo, "for-each-ref", "--format=%(refname) %(objectname)") == manifest["refs"],
        "Refs changed",
    )
    require(
        sorted(git(repo, "rev-list", "--all").splitlines()) == manifest["all_commits"],
        "Reachable history changed",
    )
    require(
        git(repo, "rev-list", "--reverse", "HEAD").splitlines() == manifest["commits"],
        "Commit graph changed",
    )
    require(len(set(manifest["commits"])) == len(assignments) == 100, "Invalid fixture")
    require([a["commit"] for a in assignments] == manifest["commits"], "Invalid fixture")
    require(not git(repo, "diff", "--cached", "--name-only"), "Changes were staged")
    require(not git(repo, "ls-files", "--others"), "Unexpected untracked files")
    status = git(repo, "diff", "--name-status").splitlines()
    require(sorted(status) == sorted("M\t" + a["module"] for a in assignments), "Wrong final diff")
    results = []
    for a in assignments:
        module = repo / a["module"]
        require(
            digest(Path(a["review_cwd"]) / a["module"]) == a["original_sha256"], "Original changed"
        )
        require(git(a["review_cwd"], "rev-parse", "HEAD") == a["commit"], "Review checkout moved")
        require(digest(repo / a["contract_path"]) == a["contract_sha256"], "Contract changed")
        result = check(module, a["contract"]["family"])
        require(result["passed"], f"{a['id']}: {result['diagnostic']}")
        require(digest(module) != a["original_sha256"], "Faulty module was not changed")
        results.append({"id": a["id"], "sha256": digest(module), **result})
    return results


def preflight(manifest):
    repo = Path(manifest["repo"])
    require(git(repo, "rev-parse", "HEAD") == manifest["head"], "Prepared HEAD changed")
    require(
        git(repo, "for-each-ref", "--format=%(refname) %(objectname)") == manifest["refs"],
        "Prepared refs changed",
    )
    require(
        git(repo, "rev-list", "--reverse", "HEAD").splitlines() == manifest["commits"]
        and sorted(git(repo, "rev-list", "--all").splitlines()) == manifest["all_commits"],
        "Prepared history changed",
    )
    require(
        not git(repo, "status", "--porcelain", "--untracked-files=all")
        and not git(repo, "ls-files", "--others"),
        "Prepared working tree is not clean",
    )
    require(
        len(manifest["assignments"]) == len(set(manifest["commits"])) == 100,
        "Expected 100 original commits",
    )
    for a in manifest["assignments"]:
        review = Path(a["review_cwd"])
        require(
            digest(repo / a["module"]) == digest(review / a["module"]) == a["original_sha256"],
            "Prepared original changed",
        )
        require(
            digest(repo / a["contract_path"])
            == digest(review / a["contract_path"])
            == a["contract_sha256"],
            "Prepared contract changed",
        )
        require(git(review, "rev-parse", "HEAD") == a["commit"], "Prepared review checkout changed")
        require(
            not check(review / a["module"], a["contract"]["family"])["passed"],
            "Prepared function is not faulty",
        )
