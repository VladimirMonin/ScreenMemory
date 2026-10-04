import unittest
import prerequisites

class Prerequisites(unittest.TestCase):
    def test_actual_existing_pinned_prerequisites_and_tracked_helpers(self):
        self.assertEqual(prerequisites.check()['packages'],prerequisites.VERSIONS)
    def test_missing_wrong_version_and_machine_refuse_before_side_effects(self):
        for version,machine,packages in [((3,14,1),'AMD64',prerequisites.VERSIONS),((3,14,2),'ARM64',prerequisites.VERSIONS),((3,14,2),'AMD64',{}),((3,14,2),'AMD64',dict(playwright='1.57.0',jsonschema='4.26.0'))]:
            with self.assertRaises(RuntimeError):prerequisites.validate(version,machine,packages)

if __name__=='__main__':unittest.main()
