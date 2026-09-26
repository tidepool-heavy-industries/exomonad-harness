{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE OverloadedStrings #-}

-- | A reusable standalone acceptance plan: Engine, Store and browser checks.
module SessionHelpers.AcceptancePlan (standaloneChecks, startAcceptancePlan) where

import Control.Monad.Freer (Eff, Member)
import qualified Tidepool.Command as Cmd
import Tidepool.Actors.Exomonad (AgentRef)
import Tidepool.Effects.Core (Actor, Commands)
import Tidepool.Worktree (GitOid, renderGitOid)
import Project.CheckResults (CheckSetupIssue)
import SessionHelpers.TestEvidence

standaloneChecks :: [PlanCheck]
standaloneChecks =
  [ engine "engine injected context"
      "injected_message_is_attempt_only_after_history_and_persists_on_transport_failure"
  , engine "store injected decision reopen"
      "inject_before_request_decision_roundtrips_after_reopen"
  , engine "invalid injected item cleanup"
      "invalid_injected_item_fails_before_transport_and_cleans_claim"
  , engine "restricted name cleanup"
      "restricted_invalid_names_and_excluded_finalize_fail_before_transport_and_cleanup_claims"
  , plannedCheck
      (CheckDefinition "standalone browser acceptance" "harness-demo"
        "test:standalone_browser"
        "standalone_missing_assets_and_clean_and_process_loss_reopen" 1)
      (Cmd.GiB 4)
      (PrepareWith $ \candidate ->
        ["bash", "scripts/prepare-browser-check", renderGitOid candidate])
  ]
  where
    engine name filterName = plannedCheck
      (CheckDefinition name "harness" "lib" filterName 1)
      (Cmd.GiB 2) WithoutPreparation

-- | Supply the integrated candidate once. Every original job remains in
-- 'planStarts'; 'readCheckPlan' gives one aggregate result later.
startAcceptancePlan
  :: (Member Actor effects, Member Commands effects)
  => AgentRef -> GitOid -> Eff effects (Either CheckSetupIssue PlanStart)
startAcceptancePlan owner candidate = startCheckPlan owner candidate standaloneChecks
