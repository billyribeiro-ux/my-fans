"""Offline schema/policy checks and adversarial controls for cloud runtime stacks."""
import copy
import json
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parent.parent
RULES = ROOT / 'deploy/aws/runtime.guard'


def tool(name):
    local = pathlib.Path(sys.executable).parent / name
    return str(local) if local.exists() else shutil.which(name) or name


def guard(document, expected):
    with tempfile.TemporaryDirectory(prefix='myfans-cloud-check-') as directory:
        path = pathlib.Path(directory) / 'template.json'
        path.write_text(json.dumps(document))
        result = subprocess.run([tool('cfn-guard'), 'validate', '--rules', str(RULES),
                                 '--data', str(path), '--output-format', 'json',
                                 '--show-summary', 'none'], capture_output=True, text=True)
        wanted = 0 if expected else 19
        if result.returncode != wanted:
            sys.exit(f'Cloud policy validation failed ({result.returncode}, expected {wanted}):\n{result.stdout}\n{result.stderr}')


documents = {}
for name in ['registry', 'foundation', 'runtime', 'edge', 'recovery-access', 'key-rotation', 'privacy-access', 'github-release-role']:
    path = ROOT / f'deploy/aws/{name}.json'
    subprocess.run([tool('cfn-lint'), '--format', 'json', '--regions', 'us-east-1',
                    '--template', str(path)], check=True)
    document = json.loads(path.read_text())
    # deploy-aws serializes JSON compactly before create-change-set TemplateBody.
    size = len(json.dumps(document, separators=(',', ':')).encode())
    if size > 51200:
        sys.exit(f'{name} compact request exceeds inline CloudFormation limit')
    if document['Metadata']['AWSToolsMetrics']['AWSAgentToolkit'] != 'aws-cloudformation@3':
        sys.exit('CloudFormation provenance marker missing')
    guard(document, True)
    documents[name] = document
    print(f'{name}: {len(document["Resources"])} resources, {size} request bytes')

mutations = [
    ('runtime', ['WorkerTask','DeletionPolicy'], 'Delete'),
    ('runtime', ['WorkerTask','UpdateReplacePolicy'], 'Delete'),
    ('runtime', ['MigrationTask','DeletionPolicy'], 'Delete'),
    ('runtime', ['MigrationTask','UpdateReplacePolicy'], 'Delete'),
    ('runtime', ['ActiveDbCpuAlarm','Properties','Dimensions',0,'Value','Ref'], 'DatabaseHost'),
    ('runtime', ['WorkerTask','Properties','ContainerDefinitions',1,'Secrets',8,'ValueFrom'], {'Ref': 'MfaKeyArn'}),
    ('privacy-access', ['PrivacyJournalExportRole','Properties','Policies',0,'PolicyDocument','Statement',0,'Action'], 's3:*'),
    ('recovery-access', ['PrivacyJournalReplayRole','Properties','Policies',0,'PolicyDocument','Statement',0,'Resource'], '*'),
    ('github-release-role', ['GitHubReleaseRole','Properties','AssumeRolePolicyDocument','Statement',0,'Condition','StringEquals','token.actions.githubusercontent.com:sub'], '*'),
    ('privacy-access', ['PrivacyVersionRole','Properties','Policies',0,'PolicyDocument','Statement',3,'Action'], 's3:*'),
    ('registry', ['ApplicationRepository', 'Properties', 'ImageTagMutability'], 'MUTABLE'),
    ('registry', ['ScannerRepository', 'Properties', 'ImageScanningConfiguration', 'ScanOnPush'], False),
    ('key-rotation', ['NextKey', 'DeletionPolicy'], 'Delete'),
    ('foundation', ['Database', 'Properties', 'PubliclyAccessible'], True),
    ('foundation', ['Database', 'Properties', 'MultiAZ'], False),
    ('foundation', ['Database', 'Properties', 'ManageMasterUserPassword'], False),
    ('foundation', ['Database', 'Properties', 'BackupRetentionPeriod'], 0),
    ('foundation', ['RuntimePassword', 'DeletionPolicy'], 'Delete'),
    ('foundation', ['DataKey', 'Properties', 'EnableKeyRotation'], False),
    ('foundation', ['RecoverySigningKey', 'DeletionPolicy'], 'Delete'),
    ('foundation', ['CloudFrontIngress', 'Properties', 'CidrIp'], '0.0.0.0/0'),
    ('foundation', ['MailQueuePolicy', 'Properties', 'PolicyDocument', 'Statement', 0, 'Condition', 'ArnEquals', 'aws:SourceArn'], '*'),
    ('runtime', ['WebTask', 'Properties', 'ContainerDefinitions', 1, 'User'], '0'),
    ('runtime', ['WorkerTask', 'Properties', 'ContainerDefinitions', 2, 'MountPoints', 0, 'SourceVolume'], 'runtime'),
    ('runtime', ['WebService', 'Properties', 'NetworkConfiguration', 'AwsvpcConfiguration', 'AssignPublicIp'], 'ENABLED'),
    ('runtime', ['WorkerTask', 'Properties', 'RequiresCompatibilities'], ['FARGATE']),
    ('runtime', ['WorkerCompute', 'Properties', 'ManagedInstancesProvider', 'InstanceLaunchTemplate', 'CapacityOptionType'], 'SPOT'),
    ('runtime', ['WorkerService', 'Properties', 'CapacityProviderStrategy', 0, 'CapacityProvider', 'Ref'], 'WebCompute'),
    ('runtime', ['LoadBalancer', 'Properties', 'Scheme'], 'internet-facing'),
    ('edge', ['Distribution', 'Properties', 'DistributionConfig', 'DefaultCacheBehavior', 'CachePolicyId'], 'unsafe-caching'),
    ('edge', ['VpcOrigin', 'Properties', 'VpcOriginEndpointConfig', 'OriginProtocolPolicy'], 'http-only'),
    ('edge', ['WebAcl', 'Properties', 'VisibilityConfig', 'SampledRequestsEnabled'], True),
]
for name, path, value in mutations:
    document = copy.deepcopy(documents[name])
    parent = document['Resources']
    for key in path[:-1]:
        parent = parent[key]
    parent[path[-1]] = value
    guard(document, False)
print(f'Cloud schema/policy verification passed; {len(mutations)} insecure mutations rejected. No AWS account was accessed.')
