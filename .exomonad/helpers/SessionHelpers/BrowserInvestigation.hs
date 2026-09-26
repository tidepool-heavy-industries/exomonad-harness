{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE OverloadedStrings #-}

-- | Harness-authored read-only diagnostics for failed browser preparation.
-- The preparation and test share one original job in the invoking checkout;
-- the investigator is its sole completion watcher and sends one final notice.
module SessionHelpers.BrowserInvestigation
  ( BrowserInvestigationStart (..), startBrowserInvestigation ) where

import Control.Monad.Freer (Eff, Member)
import qualified Data.Text as Text
import Project.BackgroundInvestigator
import Project.ParallelInvestigate
import Project.TestEvidence
import qualified Tidepool.Actor.Record as R
import Tidepool.Actors.Exomonad (AgentRef)
import qualified Tidepool.Command as Cmd
import Tidepool.Effects.Core (Actor, Commands)
import Tidepool.Worktree (GitOid, renderGitOid)
import SessionHelpers.TestEvidence (CheckDefinition, checkAt)

data BrowserInvestigationStart
  = BrowserCheckoutUnknown Cmd.OutputIssue
  | BrowserCheckRefused FocusedSetupIssue
  | BrowserInvestigationWatching FocusedRun (R.ActorHandle Investigator)
  deriving (Show)

-- | Supply the exact candidate once. The invoking actor obtains its own
-- checkout path and starts the prepared focused job there. No ordinary gate
-- watcher is attached, so this investigator owns the sole completion notice.
-- Later: readInvestigation watcher, then finishInvestigation watcher.
startBrowserInvestigation
  :: (Member Actor effects, Member Commands effects)
  => AgentRef -> GitOid -> Cmd.Memory -> CheckDefinition
  -> Eff effects BrowserInvestigationStart
startBrowserInvestigation owner candidate memory definition = do
  location <- Cmd.quiet $ Cmd.run (Cmd.withMemory (Cmd.MiB 64) (Cmd.argv ["pwd"]))
  case Cmd.stdout location of
    Left issue -> pure (BrowserCheckoutUnknown issue)
    Right output -> do
      let checkout = Text.strip output
      started <- startFocusedAfter memory (checkAt candidate definition)
        ["bash", "scripts/prepare-browser-check", renderGitOid candidate]
      case started of
        Left issue -> pure (BrowserCheckRefused issue)
        Right original -> BrowserInvestigationWatching original <$>
          watchFailedCheck owner "browser preparation" original (preparationProbes checkout) choose
  where
    preparationProbes checkout focused = case focusedPreparation focused of
      PreparationFailed _ -> probes checkout
      PreparationUnknown -> probes checkout
      _ -> []
    probes checkout =
      [ CommandProbe "assets" "built browser assets exist" checkout (Cmd.MiB 64)
          (Cmd.argv ["sh", "-c", "if test -f web/dist/index.html; then printf present; else printf absent; fi"])
      , CommandProbe "source" "checkout source and tracked changes" checkout (Cmd.MiB 64)
          (Cmd.argv ["git", "status", "--short"])
      ]
    choose _ available = pure (Right (case available of
      [] -> Nothing
      first : _ -> Just first))
