#!/usr/bin/env python3
"""Prove generated contract drift fails without mutating the working tree."""
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest

ROOT=pathlib.Path(__file__).resolve().parent.parent

class ContractDriftTests(unittest.TestCase):
    def run_generator(self,root,check=False):
        return subprocess.run([sys.executable,str(root/'protocol/generate.py')]+(['--check'] if check else []),cwd=root,capture_output=True,text=True)
    def fixture(self,directory):
        root=pathlib.Path(directory)
        shutil.copytree(ROOT/'protocol',root/'protocol')
        for target in ['SoundroomCore','SoundroomRemote']:
            shutil.copytree(ROOT/'Sources'/target,root/'Sources'/target)
        generated_test='Tests/SoundroomRemoteTests/RemoteV3EnumParityTests.swift'
        (root/generated_test).parent.mkdir(parents=True)
        shutil.copy2(ROOT/generated_test,root/generated_test)
        return root
    def test_escaped_enum_cases_and_coding_keys_preserve_native_wire_values(self):
        from model_contract import extract
        with tempfile.TemporaryDirectory(prefix='soundroom-enum-fixture-') as directory:
            source=pathlib.Path(directory)/'Fixture.swift'
            source.write_text('public enum Role: String, Codable { case viewer, `operator`, renamed = "engineer", blank = "" }\npublic struct Alias: Codable {\n    public var name: String\n    enum CodingKeys: String, CodingKey { case name = "operator" }\n}\nstruct PrivateCredential: Codable { let secret: String }')
            records,enums=extract([source])
            self.assertEqual(enums['Role'],['viewer','operator','engineer',''])
            self.assertEqual(records['Alias']['fields'][0]['name'],'operator')
            self.assertNotIn('PrivateCredential',records)
    def test_checked_in_contract_matches_sources(self):
        result=self.run_generator(ROOT,True)
        self.assertEqual(result.returncode,0,result.stderr+result.stdout)
    def test_wrapped_associated_values_preserve_the_contract(self):
        import re
        with tempfile.TemporaryDirectory(prefix='soundroom-contract-layout-') as directory:
            root=self.fixture(directory)
            path=root/'Sources/SoundroomRemote/RemoteProtocol.swift'
            source=path.read_text()
            source,count=re.subn(r'case splitClip\(trackID: UUID, clipID: UUID, frame: Int\)',
                'case splitClip(\n        trackID: UUID,\n        clipID: UUID,\n        frame: Int\n    )',source)
            self.assertEqual(count,1)
            path.write_text(source)
            result=self.run_generator(root,True)
            self.assertEqual(result.returncode,0,result.stderr+result.stdout)
    def test_wrapped_model_headers_defaults_and_decode_calls_preserve_fields(self):
        from model_contract import extract
        with tempfile.TemporaryDirectory(prefix='soundroom-model-layout-') as directory:
            path=pathlib.Path(directory)/'Fixture.swift'
            path.write_text('''public enum Role:
    String, Codable, CaseIterable
{
    case viewer,
        editor
}
public struct Fixture:
    Codable, Sendable
{
    public var name: String
    public var label = "("
    public var values: [Int] =
        [1,
         2]
    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(
            String.self,
            forKey: .name
        )
    }
}''')
            records,enums=extract([path])
            self.assertEqual(enums['Role'],['viewer','editor'])
            fields=records['Fixture']['fields']
            self.assertEqual([(f['name'],f['type'],f['required']) for f in fields],
                [('name','String',True),('label','String',False),('values','[Int]',False)])
    def test_operation_registry_changes_require_regeneration(self):
        import json
        with tempfile.TemporaryDirectory(prefix='soundroom-operation-fixture-') as directory:
            root=self.fixture(directory)
            path=root/'protocol/editor_operations.json'
            names=json.loads(path.read_text())
            path.write_text(json.dumps(sorted(names+['test.fixtureOperation'])))
            result=self.run_generator(root,True)
            self.assertNotEqual(result.returncode,0)
    def test_modified_artifact_is_rejected(self):
        with tempfile.TemporaryDirectory(prefix='soundroom-contract-drift-') as directory:
            root=self.fixture(directory)
            artifact=root/'protocol/generated/contract.ts'
            artifact.write_text(artifact.read_text()+'\n// unreviewed drift\n')
            result=self.run_generator(root,True)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('Generated contract drift',result.stderr+result.stdout)
    def test_new_serialized_field_requires_regeneration(self):
        with tempfile.TemporaryDirectory(prefix='soundroom-contract-drift-') as directory:
            root=self.fixture(directory)
            source=root/'Sources/SoundroomRemote/RemoteCapabilities.swift'
            text=source.read_text().replace('public struct RemoteCapabilities: Codable, Equatable, Sendable {','public struct RemoteCapabilities: Codable, Equatable, Sendable {\n    public var newContractFlag = false')
            source.write_text(text)
            result=self.run_generator(root,True)
            self.assertNotEqual(result.returncode,0)
            self.assertIn('Generated contract drift',result.stderr+result.stdout)
            self.assertEqual(self.run_generator(root).returncode,0)
            self.assertEqual(self.run_generator(root,True).returncode,0)

if __name__=='__main__': unittest.main()
