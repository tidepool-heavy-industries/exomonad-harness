{-# LANGUAGE FlexibleContexts #-}

-- | Remix this session seed for the current component. Keep evidence parsing
-- and acceptance rules in the shared owner; specialize commands and policy here.
module SessionHelpers.TestEvidence
  ( module Project.TestEvidence, runTests
  ) where

import Control.Monad.Freer (Eff, Member)
import Project.TestEvidence
import qualified Tidepool.Command as Cmd
import Tidepool.Effects.Core (Commands)

-- Start once, retain the result, then compose watchChecks or collectFocused.
-- Tune this reservation and specialize a FocusedSpec for the current work.
runTests
  :: Member Commands effects
  => FocusedSpec -> Eff effects (Either FocusedSetupIssue FocusedRun)
runTests = startFocused (Cmd.GiB 4)
