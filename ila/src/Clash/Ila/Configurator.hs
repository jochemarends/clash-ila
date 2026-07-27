{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE OverloadedRecordDot #-}
{-# LANGUAGE OverloadedStrings #-}
{-# LANGUAGE TypeAbstractions #-}
{-# LANGUAGE UndecidableInstances #-}
{-# LANGUAGE NoFieldSelectors #-}
{-# LANGUAGE UndecidableSuperClasses #-}
{-# LANGUAGE AllowAmbiguousTypes #-}

module Clash.Ila.Configurator (
  -- | ILA configuration
  ilaConfig,
  IlaConfig(..),
  WithIlaConfig(..),

  -- | The default list of predicates
  ilaPredicateEq,
  ilaPredicateGt,
  ilaPredicateLt,
  ilaPredicateTrue,
  ilaPredicateFalse,
  ilaDefaultPredicates,
  -- | Types for custom predicates
  Predicate,
  NamedPredicate,
  RawPredicate,

  -- | Clash refuses to compile if these blackbox functions are not in scope
  writeSignalInfo,
  signalInfoBBF,

  genShockwavesMetadata,
) where

import Clash.Prelude hiding (Exp, Type, traceSignal, dumpVCD)
import Prelude qualified as P

import Clash.Annotations.Primitive
import Clash.Backend
import Clash.Core.Term
import Clash.Core.TermLiteral
import Clash.Core.Type
import Clash.Netlist.BlackBox.Types
import Clash.Netlist.Types
import Clash.Primitives.DSL qualified as DSL

import Control.Lens (view)
import Control.Monad.State (State)
import Data.String.Interpolate (__i)
import GHC.Stack (HasCallStack)
import Prettyprinter (Doc, pretty)

import Data.Aeson (ToJSON (..))
import Data.Aeson qualified
import Data.Aeson.Text (encodeToLazyText)
import Data.Either
import Data.Hashable (Hashable, hash)
import Data.Kind qualified
import Data.Monoid (Ap (getAp))
import Data.Word (Word32)

import Clash.Ila.Internal.Shockwaves (Metadata (..), metadataRef, updateMetadata)
import Clash.Shockwaves qualified as Shockwaves
import Data.IORef
import GHC.IO
import Data.Data (Proxy(..))

{- | ILA predicate function
Used to compare incoming data to a reference value (with a possible mask) and returns if the
predicate holds or not. This is used for the ILA trigger and the ILA capture
-}
type Predicate dom a = Signal dom (RawPredicate a)

-- | Internal predicate type that gets passed to the ILA
type RawPredicate a =
  -- | Incoming sample
  a ->
  -- | Data to compare too
  BitVector (BitSize a) ->
  -- | Sample mask
  BitVector (BitSize a) ->
  -- | Bool indicating if the predicate holds true
  Bool

-- | Turns a combinational function into a predicate
simplePredicate :: forall dom a.
  (a -> BitVector (BitSize a) -> BitVector (BitSize a) -> Bool) ->
  Predicate dom a
simplePredicate f = pure f

-- | Same as `Predicate dom a` but with a string to display in the CLI
type NamedPredicate dom a = (Predicate dom a, String)

{- | Default ILA predicate for checking equality
It applies the mask over the incoming sample and `==` it with the compare value
-}
ilaPredicateEq :: forall dom a. (BitPack a) => Predicate dom a
ilaPredicateEq = simplePredicate ilaPredicateEq'
  where
    ilaPredicateEq' s c m = m /= 0 && ((pack s) .&. m) == c

{- | Default ILA predicate for checking less-than
It applies the mask over the incoming sample and `<` it with the compare value
-}
ilaPredicateLt :: forall dom a. (BitPack a) => Predicate dom a
ilaPredicateLt = simplePredicate ilaPredicateLt'
  where
    ilaPredicateLt' s c m = m /= 0 && ((pack s) .&. m) < c

{- | Default ILA predicate for checking greater-than
It applies the mask over the incoming sample and `>` it with the compare value
-}
ilaPredicateGt :: forall dom a. (BitPack a) => Predicate dom a
ilaPredicateGt = simplePredicate ilaPredicateGt'
  where
    ilaPredicateGt' s c m = m /= 0 && ((pack s) .&. m) > c

-- | Default ILA predicate, ignores all arguments and always returns `True`
ilaPredicateTrue :: forall dom a. (BitPack a) => Predicate dom a
ilaPredicateTrue = simplePredicate ilaPredicateTrue'
  where
    ilaPredicateTrue' _ _ _ = True

-- | Default ILA predicate, ignores all arguments and always returns `False`
ilaPredicateFalse :: forall dom a. (BitPack a) => Predicate dom a
ilaPredicateFalse = simplePredicate ilaPredicateFalse'
  where
    ilaPredicateFalse' _ _ _ = False

-- | Predefined list of three ILA predicates. The operators it covers are: `==`, `>`, `<`
ilaDefaultPredicates :: forall dom a. (BitPack a) => Vec 5 (NamedPredicate dom a)
ilaDefaultPredicates =
  ( (ilaPredicateEq, "Equals")
      :> (ilaPredicateGt, "Greater than")
      :> (ilaPredicateLt, "Less than")
      :> (ilaPredicateTrue, "Always true")
      :> (ilaPredicateFalse, "Always false")
      :> Nil
  )

{- | The initial ILA configuration. This record contains all the data needed to create the initial
register map of the ILA.
-}
data IlaConfig dom where
  IlaConfig ::
    forall dom a n m.
    ( KnownNat n
    , KnownNat m
    , NFDataX a
    , BitPack a
    , 1 <= BitSize a `DivRU` 32
    , 1 <= BitSize a
    , 1 <= n
    , 1 <= m
    , m <= 64
    ) =>
    { depth :: SNat n
    -- ^ The amount of samples that can be stored in the buffer
    , triggerPoint :: Index n
    -- ^ How many samples *after* triggering it will continue to sample
    , hash :: BitVector 32
    -- ^ The hash of the ILA, is used to check if the computer is talking to the right ILA
    , tracing :: Signal dom a
    -- ^ The signal being sampled from
    , predicates :: Vec m (Predicate dom a)
    -- ^ A list of predicates, used to control trigger/capture logic
    } ->
    IlaConfig dom

{- | The user provided ILA configuration record. This is used to terminate the creation of an
`IlaConfig` when using `ilaConfig`.
-}
data WithIlaConfig dom a where
  WithIlaConfig ::
    forall dom n m a.
    ( KnownNat n
    , KnownNat m
    , BitPack a
    , 1 <= BitSize a `DivRU` 32
    , 1 <= BitSize a
    , 1 <= n
    , 1 <= m
    , m <= 64
    ) =>
    { name :: String
    -- ^ The name displayed in the VCD file for your toplevel design
    , triggerPoint :: Index n
    -- ^ How many samples *after* triggering it will continue to sample
    , bufferDepth :: SNat n
    -- ^ The amount of samples that can be stored in the buffer
    , predicates :: Vec m (NamedPredicate dom a)
    -- ^ A list of predicates, used to control trigger/capture logic
    } ->
    WithIlaConfig dom a

{- | From a tuple consisting of a signal and a string, grab the bit width of the signal and put it
in a vector. At the same time, bundle every signal together.
-}
class LabeledSignals a where
  ilaProbe :: a

{- | Terminating case
Once encounting a `WithIlaConfig`, terminate collecting of signals and return the `IlaConfig`
generated by the collected signals and labeles.
-}
instance
  forall dom0 dom1 n a b.
  ( NFDataX a
  , dom0 ~ dom1
  , BitPack a
  , 1 <= BitSize a `DivRU` 32
  , a ~ b
  , KnownSymbol dom0
  ) =>
  LabeledSignals
    ((Vec n GenSignal, Signal dom0 a) -> WithIlaConfig dom1 b -> IlaConfig dom0)
  where
  ilaProbe ::
    (Vec n GenSignal, Signal dom0 a) ->
    WithIlaConfig dom1 a ->
    IlaConfig dom0
  ilaProbe (signalInfos, tracing) (WithIlaConfig @_ @_ @_ toplevel triggerPoint bufferDepth predicates) =
    alterShockwavesMetadata `seq` config
   where
    ilaHash = writeSignalInfo toplevel bufferDepth (fromGenSignal <$> signalInfos) (snd <$> predicates)
    config =
      IlaConfig
        { depth = bufferDepth
        , triggerPoint = triggerPoint
        , hash = ilaHash
        , tracing = tracing
        , predicates = fst <$> predicates
        }
    alterShockwavesMetadata = unsafePerformIO $ modifyIORef metadataRef (\m -> m { scope = Just toplevel })

{- | General case
For every pair of new set of `(Signal dom a, "name")`, bundle the signal and collect the name
into a `Vec`.
-}
instance
  forall dom a b m n next.
  ( m ~ n + 1
  , NFDataX b
  , BitPack b
  , Shockwaves.Waveform b
  , 1 <= BitSize b `DivRU` 32
  , LabeledSignals ((Vec m GenSignal, Signal dom (a, b)) -> next)
  ) =>
  LabeledSignals ((Vec n GenSignal, Signal dom a) -> (Signal dom b, String) -> next)
  where
  ilaProbe ::
    (Vec n GenSignal, Signal dom a) ->
    (Signal dom b, String) ->
    next
  ilaProbe (prevInfos, prevSignal) (newSignal, newName) =
    alterShockwavesMetadata `seq` (ilaProbe (prevInfos ++ (newInfo :> Nil), newBundled))
   where
    newInfo = GenSignal{name = newName, width = natToNum @(BitSize b)}
    newBundled = (,) <$> prevSignal <*> newSignal
    alterShockwavesMetadata =
      if clashSimulation
        then
          unsafePerformIO $ updateMetadata @b newName
        else ()

{- | A polyvariadic function containing 'labelled signals', aka, a list of tuples where the left
side is an arbitary signal, and the right a string.

# Example:

>>> counter = register 0 $ counter + 1 :: Signal System (Unsigned 8)
>>> active = pure True :: Signal System Bool
>>> probe = ilaProbe (counter, "8 bit value") (active, "system active")
>>> :t probe
>>> probe
  :: (LabelledSignals t 2 "System" (Unsigned 8, Bool),
      Hidden "clock" (Clock System), Hidden "reset" (Reset System),
      Hidden "enable" (Enable System)) =>
      t
-}
ilaConfig ::
  forall dom a next.
  ( NFDataX a
  , BitPack a
  , 1 <= BitSize a `DivRU` 32
  , LabeledSignals ((Vec 1 GenSignal, Signal dom ((), a)) -> next)
  , Shockwaves.Waveform a
  ) =>
  (Signal dom a, String) ->
  next
ilaConfig (s :: (Signal dom a, String)) =
  ilaProbe (Nil :: Vec 0 GenSignal, pure () :: Signal dom ()) s

{- | Write metadata of signals to a json file, this metadata includes the width of the signal and a
given label

The structure of the json file is defined at the `GenIla` record
-}
writeSignalInfo ::
  forall n m s.
  -- | Toplevel name
  String ->
  -- | Buffer size
  SNat s ->
  -- | A `Vec` of signal widths and their label
  Vec n (Int, String) ->
  Vec m String ->
  -- | The hash of the JSON
  BitVector 32
writeSignalInfo !_toplevel !_bufSize !_sigInfo !_triggerNames = 0
{-# OPAQUE writeSignalInfo #-}
{-# ANN writeSignalInfo hasBlackBox #-}
{-# ANN
  writeSignalInfo
  ( let
      primitive = 'writeSignalInfo
      template = 'signalInfoBBF
     in
      InlineYamlPrimitive
        [minBound ..]
        [__i|
      BlackBoxHaskell:
        name: #{primitive}
        templateFunction: #{template}
        workInfo: Always
    |]
  )
  #-}

{- | The actual blackbox function, this grabs the AST from the callstack, reconstructs the proper
types from it and creates the blackbox & blackbox meta functions (which in turn write the reconstructed
types to a file)
-}
signalInfoBBF :: (HasCallStack) => BlackBoxFunction
signalInfoBBF _ _ args _ = view tcCache >>= go
 where
  go tcm
    | [toplevel, _, sigInfo, triggerNames] <- lefts args
    , [ (coreView tcm -> LitTy (NumTy n))
        , (coreView tcm -> LitTy (NumTy m))
        , (coreView tcm -> LitTy (NumTy s))
        ] <-
        rights args
    , Just (SomeNat (Proxy :: Proxy n)) <- someNatVal n
    , Just (SomeNat (Proxy :: Proxy m)) <- someNatVal m
    , Just (SomeNat (Proxy :: Proxy s)) <- someNatVal s =
        mkBlackBox
          $ getGenIla @n @m @s
            toplevel
            (SNat @s)
            (getSigInfo sigInfo)
            (coerceTermToType triggerNames)
    | otherwise =
        errorX
          "AST does not match expected, expected AST in the form of String -> SNat -> Vec n (Int, String)"

  -- \| Make the actual blackbox
  mkBlackBox input = pure $ Right (blackBoxMeta input, blackBox input)

  -- \| Coerce a `Term` back into a type
  -- Panics on failure
  coerceTermToType ::
    (TermLiteral a) =>
    -- \| The AST of the type
    Term ->
    -- \| The result type
    a
  coerceTermToType term = either errorX id $ termToDataError term

  -- \| Get the signal information from it's AST form
  getSigInfo :: forall n. (KnownNat n) => Term -> Vec n (Int, String)
  getSigInfo term = coerceTermToType term

  -- \| Generate the ILA from the AST
  getGenIla ::
    forall n m s.
    ( KnownNat n
    , KnownNat m
    , KnownNat s
    ) =>
    Term ->
    SNat s ->
    Vec n (Int, String) ->
    Vec m (String) ->
    GenIla
  getGenIla toplevel bufSize sigInfo triggerNames =
    GenIla
      { toplevel = coerceTermToType toplevel
      , bufferSize = snatToNum bufSize
      , hash =
          fromIntegral
            $ hash
              ( coerceTermToType toplevel :: String
              , snatToInteger bufSize
              , toList sigInfo
              )
      , -- The reverse is needed as the polyvariadic function builds up the vector in reverse order
        signals = P.reverse $ toList $ toGenSignal <$> sigInfo
      , triggerNames = toList triggerNames
      }

  -- \| Meta information about the blackbox
  -- We abuse the `bbIncludes` feature to write our ILA configuration to a JSON file
  blackBoxMeta :: GenIla -> BlackBoxMeta
  blackBoxMeta sizes =
    emptyBlackBoxMeta
      { bbKind = Clash.Netlist.BlackBox.Types.TExpr
      , bbIncludes =
          [
            ( ("ilaconf", "json")
            , BBFunction (show 'renderJSONTF) 0 (renderJSONTF sizes)
            )
          ]
      }

  -- \| The blackbox itself, which we don't use as we only want to write to meta files
  blackBox :: GenIla -> BlackBox
  blackBox sizes = BBFunction (show 'renderHDLTF) 0 (renderHDLTF sizes)
{-# NOINLINE signalInfoBBF #-}

{- | Template function to generate HDL
As we only want to write JSON, this is simply an empty string
-}
renderHDLTF :: (HasCallStack) => GenIla -> TemplateFunction
renderHDLTF args = TemplateFunction [] (const True) (renderHDL args)
{-# NOINLINE renderHDLTF #-}

-- | Template function to generate JSON
renderJSONTF :: (HasCallStack) => GenIla -> TemplateFunction
renderJSONTF args = TemplateFunction [] (const True) (renderJSON args)
{-# NOINLINE renderJSONTF #-}

-- | Calculate the hash over the file contents and return it as a number
renderHDL ::
  forall s.
  ( HasCallStack
  , Backend s
  ) =>
  -- | The Ilas to encode to get the hash from
  GenIla ->
  -- | Unused
  BlackBoxContext ->
  -- | The output JSON content
  State s (Doc ())
renderHDL ila _ = getAp $ expr True (DSL.bvLit 32 $ toInteger ila.hash).eex

-- | Actually render JSON
renderJSON ::
  forall s.
  ( HasCallStack
  , Backend s
  ) =>
  -- | The Ilas to encode to JSON
  GenIla ->
  -- | Unused
  BlackBoxContext ->
  -- | The output JSON content
  State s (Doc ())
renderJSON ila _ = pure . pretty $ encodeToLazyText ila
{-# NOINLINE renderJSON #-}

-- All types primarily just used to properly format a JSON document

-- | Individual signal JSON representation
data GenSignal = GenSignal
  { name :: String
  , width :: Int
  }
  deriving (Generic, Show, ToJSON, Eq, Hashable)

toGenSignal :: (Int, String) -> GenSignal
toGenSignal (w, n) = GenSignal n w

fromGenSignal :: GenSignal -> (Int, String)
fromGenSignal s = (s.width, s.name)

type Ports = [(Data.Kind.Type, Symbol)]

type family
  ConstrainedPorts (ports :: Ports) (c :: Data.Kind.Type -> Data.Kind.Constraint) ::
    Data.Kind.Constraint
  where
  ConstrainedPorts '[] c = ()
  ConstrainedPorts ('(a, _) ': ports) c = (c a, ConstrainedPorts ports c)

type family CountPorts (ports :: Ports) :: Nat where
  CountPorts '[] = 0
  CountPorts (_ ': ports) = 1 + (CountPorts ports)

class
  ( ConstrainedPorts ports BitPack
  , ConstrainedPorts ports NFDataX
  , ConstrainedPorts ports Generic
  , BitPack (PortsTuple ports)
  , NFDataX (PortsTuple ports)
  , Generic (PortsTuple ports)
  ) =>
  KnownPorts ports
  where
  type PortsTuple ports :: Data.Kind.Type
  toGenSignals :: Vec (CountPorts ports) GenSignal

instance KnownPorts '[] where
  type PortsTuple '[] = ()
  toGenSignals = Nil

instance
  ( BitPack a
  , NFDataX a
  , Generic a
  , KnownSymbol label
  , KnownPorts ports
  ) =>
  KnownPorts ('(a, label) ': ports)
  where
  type PortsTuple ('(a, label) ': ports) = (a, PortsTuple ports)
  toGenSignals = genSignal :> (toGenSignals @ports)
   where
    genSignal = GenSignal{name = symbolVal (Proxy @label), width = natToNum @(BitSize a)}

-- | Individual ILA JSON representation
data GenIla = GenIla
  { toplevel :: String
  , bufferSize :: Word32
  , hash :: Word32
  , signals :: [GenSignal]
  , triggerNames :: [String]
  }
  deriving (Generic, Show, ToJSON, Eq, Hashable)

-- | Generate a metadata file for Clash Shockwaves for the ILA's signals.
--
-- The passed signal should depend on the ILA-core, which gets sampled to ensure
-- the metadata gets populated.
genShockwavesMetadata :: forall dom a. (KnownDomain dom, NFDataX a) => FilePath -> Signal dom a -> IO ()
genShockwavesMetadata path s = do
  _ <- mapM_ (evaluate . rnfX) (sampleN 100 s)
  metadata <- readIORef metadataRef
  Data.Aeson.encodeFile path metadata
