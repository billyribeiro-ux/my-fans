"""Validate infrastructure without AWS credentials or resource mutations."""

import copy
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile

from cfnlint.decode import decode

ROOT = pathlib.Path(__file__).resolve().parent.parent
TEMPLATE = ROOT / "deploy/aws/media.yaml"
RULES = ROOT / "deploy/aws/media.guard"


def tool(name):
    executable = pathlib.Path(sys.executable).parent / name
    return str(executable) if executable.exists() else shutil.which(name) or name


def guard(document, expected):
    with tempfile.TemporaryDirectory(prefix="myfans-cfn-") as directory:
        path = pathlib.Path(directory) / "template.json"
        path.write_text(json.dumps(document))
        result = subprocess.run(
            [tool("cfn-guard"), "validate", "--rules", str(RULES), "--data", str(path),
             "--output-format", "json", "--show-summary", "none"],
            capture_output=True, text=True, check=False,
        )
        if expected:
            if result.returncode != 0:
                sys.exit(f"Guard rejected the media template:\n{result.stdout}\n{result.stderr}")
        else:
            # Guard uses 19 for policy noncompliance; parsing/tool failures must
            # not masquerade as successful negative-control tests.
            if result.returncode != 19:
                sys.exit(f"Guard failed to reject an insecure mutation: {result.returncode}\n{result.stdout}\n{result.stderr}")


subprocess.run(
    [tool("cfn-lint"), "--format", "json", "--regions", "us-east-1", "eu-west-1",
     "--template", str(TEMPLATE)], check=True,
)
document, errors = decode(str(TEMPLATE))
if errors or not document:
    sys.exit("Cannot decode CloudFormation template")
if TEMPLATE.stat().st_size > 51200:
    sys.exit("Template exceeds the inline CloudFormation limit")
guard(document, True)

mutations = [
    ("MediaBucket", ["Properties", "PublicAccessBlockConfiguration", "BlockPublicPolicy"], False),
    ("RecoveryBucket", ["Properties", "VersioningConfiguration", "Status"], "Suspended"),
    ("AuditBucket", ["DeletionPolicy"], "Delete"),
    ("MediaBucketPolicy", ["DeletionPolicy"], "Delete"),
    ("RecoveryKey", ["Properties", "EnableKeyRotation"], False),
    ("MediaAuditTrail", ["Properties", "EnableLogFileValidation"], False),
    ("MediaBucket", ["Properties", "ReplicationConfiguration", "Rules", 0, "DeleteMarkerReplication", "Status"], "Enabled"),
    ("MediaBucketPolicy", ["Properties", "PolicyDocument", "Statement", 0, "Effect"], "Allow"),
    ("MediaTaskRole", ["Properties", "Policies", 0, "PolicyDocument", "Statement", 1, "Action"], ["s3:*"]),
    ("MediaTaskRole", ["Properties", "AssumeRolePolicyDocument", "Statement", 0, "Condition", "StringEquals", "aws:SourceAccount"], "*"),
]
for resource, path, value in mutations:
    mutated = copy.deepcopy(document)
    parent = mutated["Resources"][resource]
    for key in path[:-1]:
        parent = parent[key]
    parent[path[-1]] = value
    guard(mutated, False)
print(f"Infrastructure validation passed: schema, nine policy rules, {len(mutations)} insecure mutations rejected. No AWS account operations were performed.")
