"""Pure readiness schedules only; never native-window or input proof."""
import unittest
from inspect_search import wait_for_endpoint, endpoint_outcome

NOT_FOUND = 'CmdletizationQuery_NotFound,Get-NetTCPConnection'

class Clock:
    def __init__(self): self.now = 0.0
    def __call__(self): return self.now
    def sleep(self, duration): self.now += duration

class ReadinessTests(unittest.TestCase):
    def run_schedule(self, outcomes, guard=lambda remaining: None, budget=15):
        clock = Clock()
        observed = []
        def query(remaining):
            observed.append(remaining)
            result = outcomes.pop(0) if len(outcomes) > 1 else outcomes[0]
            if isinstance(result, Exception): raise result
            return result
        result = wait_for_endpoint(guard, query, clock=clock, sleep=clock.sleep, budget=budget)
        return result, clock, observed

    def test_not_ready_then_owned_rechecks_guards_before_return(self):
        guards=[]
        result, clock, observed = self.run_schedule(['not_ready', 'owned'], lambda r: guards.append(r))
        self.assertEqual(result, 2)
        self.assertEqual(clock.now, .5)
        self.assertEqual(len(guards), 3) # pre-query twice + final pre-CDP guard
        self.assertEqual(observed, [15, 14.5])

    def test_deadline_is_total_including_queries_guards_intervals(self):
        clock=Clock(); calls=[]
        def guard(r): clock.now += .1
        def query(r): calls.append(r); clock.now += .4; return 'not_ready'
        with self.assertRaises(TimeoutError):
            wait_for_endpoint(guard, query, clock=clock, sleep=clock.sleep, budget=1.5)
        self.assertLessEqual(clock.now, 1.5)
        self.assertEqual(len(calls), 2)
        self.assertLess(calls[1], calls[0])

    def test_hung_query_is_immediate_refusal_without_retry(self):
        with self.assertRaises(TimeoutError): self.run_schedule([TimeoutError('owned query timeout'), 'owned'])

    def test_errors_empty_malformed_foreign_never_wait(self):
        for raw, payload in [(1, {'outcome':'not_ready','fully_qualified_error_id':NOT_FOUND}),
                             (3, {'outcome':'not_ready','fully_qualified_error_id':'AccessDenied'}),
                             (0, {}), (0, {'outcome':'foreign'}), (3, {'outcome':'owned'})]:
            with self.subTest(raw=raw, payload=payload), self.assertRaises(RuntimeError):
                endpoint_outcome(raw, payload)
        self.assertEqual(endpoint_outcome(3, {'outcome':'not_ready','fully_qualified_error_id':NOT_FOUND}), 'not_ready')
        self.assertEqual(endpoint_outcome(0, {'outcome':'owned'}), 'owned')

    def test_gui_exit_mutated_tuple_pid_reuse_refuse_before_query_or_cdp(self):
        for defect in ['GUI exit', 'config mutation', 'PID reuse', 'image mutation', 'argv mutation', 'reparse']:
            calls=[]
            def guard(r): raise RuntimeError(defect)
            with self.subTest(defect=defect), self.assertRaises(RuntimeError):
                wait_for_endpoint(guard, lambda r: calls.append('query'), clock=Clock(), sleep=lambda s: None)
            self.assertEqual(calls, [])

    def test_identity_change_after_owned_still_refuses_before_cdp(self):
        calls=[]
        def guard(r):
            calls.append('guard')
            if len(calls)==3: raise RuntimeError('PID reuse before CDP')
        def query(r): calls.append('query'); return 'owned'
        with self.assertRaises(RuntimeError): wait_for_endpoint(guard, query, clock=Clock(), sleep=lambda s: None)
        self.assertEqual(calls, ['guard','query','guard'])

    def test_late_owned_or_exhausted_guard_cannot_pass(self):
        for phase in ['query', 'guard']:
            clock=Clock()
            def guard(r):
                if phase=='guard': clock.now += r
            def query(r):
                if phase=='query': clock.now += r
                return 'owned'
            with self.subTest(phase=phase), self.assertRaises(TimeoutError):
                wait_for_endpoint(guard, query, clock=clock, sleep=clock.sleep)

if __name__ == '__main__': unittest.main()
