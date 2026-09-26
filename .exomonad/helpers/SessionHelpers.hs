-- Remix the seed for repeated project work; keep parsing and acceptance in
-- the shared owners. startGate composes runTests-style launch with a watcher.
module SessionHelpers
  ( module SessionHelpers.TestEvidence
  , module SessionHelpers.BrowserChecks
  , module SessionHelpers.AcceptancePlan
  , module SessionHelpers.BrowserInvestigation
  , module Project.FocusedGateExample
  ) where

import SessionHelpers.TestEvidence
import SessionHelpers.BrowserChecks
import SessionHelpers.AcceptancePlan
import SessionHelpers.BrowserInvestigation
import Project.FocusedGateExample
