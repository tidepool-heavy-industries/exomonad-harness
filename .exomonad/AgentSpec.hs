{-# LANGUAGE FlexibleContexts #-}
{-# LANGUAGE OverloadedStrings #-}

module AgentSpec (agentSpec) where

import Control.Monad.Freer (Member)
import Tidepool.Agent.Contract
import qualified Project.Tools as Tools
import Tidepool.Effects.Core (ActorContext, Commands, Jev, Journal, Lookup, Notifications, Reflect)
import qualified Project.Nudges as Nudges

-- The project's own after-tool nudge layer (docs/nudges.md), installed for
-- every actor regardless of label -- see Project.Nudges's module header for
-- what it covers and what it defers. It supersedes the vendored
-- Project.Watchdog as this project's afterTool: Nudges' "everyone" battery
-- already carries Watchdog's core heuristics (repeating_itself,
-- ignoring_a_failure, destructive_command), so installing both would ask
-- Jev the same questions twice per call.
agentSpec ::
  ( Member Commands effects, Member Lookup effects, Member Jev effects
  , Member ActorContext effects, Member Notifications effects, Member Reflect effects
  , Member Journal effects
  ) =>
  AgentSpec Tools.WorkspaceTools effects
agentSpec = defaultSpec
  { specTools = Tools.tools
  , afterTool = Just Nudges.watch
  }
