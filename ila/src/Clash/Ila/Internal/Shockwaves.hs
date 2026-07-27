{-# LANGUAGE AllowAmbiguousTypes #-}
{-# LANGUAGE OverloadedStrings #-}
{-# LANGUAGE RecordWildCards #-}

module Clash.Ila.Internal.Shockwaves where

import Clash.Shockwaves.Internal.Waveform (addTypes, typeName)
import Clash.Shockwaves.Waveform (Translator, Waveform)

import Clash.Prelude
import Data.Aeson (ToJSON (..), object, (.=))
import Data.IORef
import Data.Map qualified as M
import GHC.IO

-- | Shockwaves metadata
data Metadata = Metadata
  { scope :: Maybe String
  , signals :: M.Map String String
  , types :: M.Map String Translator
  }
  deriving (Generic, Default)

instance ToJSON Metadata where
  toJSON Metadata{..} =
    object
      [ "signals" .= M.mapKeys (\k -> maybe k (<> "." <> k) scope) signals
      , "types" .= types
      , "luts" .= object []
      ]

-- | Shockwaves metadata for `Ila.Configurator` to use.
metadataRef :: IORef Metadata
metadataRef = unsafePerformIO (newIORef def)

-- | Update Shockwaves metadata to mark the signal named `name` as having this waveform.
updateMetadata :: forall a. (Waveform a) => String -> IO ()
updateMetadata name =
  modifyIORef
    metadataRef
    ( \metadata@Metadata{..} ->
        metadata
          { signals = M.insert name (typeName @a) signals
          , types = addTypes @a types
          }
    )
