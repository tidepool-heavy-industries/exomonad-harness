{-# LANGUAGE OverloadedStrings #-}
{-# LANGUAGE FlexibleContexts #-}

-- Preparation and the assertion share the invoking actor's original job and
-- checkout. The observer reads retained evidence, never another actor's files.
module SessionHelpers.BrowserChecks (runBrowserCheck, standaloneBrowser) where

import Control.Monad.Freer (Eff, Member)
import Project.FocusedGateExample (GateStart, startPreparedGate)
import SessionHelpers.TestEvidence
import Tidepool.Actors.Exomonad (AgentRef)
import Tidepool.Effects.Core (Actor, Commands)
import Tidepool.Worktree (GitOid, renderGitOid)
import qualified Tidepool.Command as Cmd

standaloneBrowser :: CheckDefinition
standaloneBrowser = CheckDefinition "standalone browser acceptance" "harness-demo"
  "test:standalone_browser" "standalone_missing_assets_and_clean_and_process_loss_reopen" 1

runBrowserCheck :: (Member Actor effects, Member Commands effects) => AgentRef -> GitOid -> Cmd.Memory -> CheckDefinition -> Eff effects GateStart
runBrowserCheck owner candidate memory definition =
  startPreparedGate owner (checkIntent definition) memory (checkAt candidate definition)
    [ "bash", "scripts/prepare-browser-check", renderGitOid candidate ]
