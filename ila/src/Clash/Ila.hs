{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE OverloadedRecordDot #-}
{-# LANGUAGE TypeAbstractions #-}
{-# LANGUAGE UndecidableInstances #-}
{-# LANGUAGE NoFieldSelectors #-}
{-# OPTIONS_GHC -fconstraint-solver-iterations=10 #-}
{-# OPTIONS_GHC -fplugin=Protocols.Plugin #-}

module Clash.Ila (
  ilaWb,
  ila,
  ilaUart,
) where

import Clash.Prelude

import Clash.Ila.Configurator
import Clash.Ila.Internal.RingBuffer
import Clash.Ila.Internal.Communication

import Clash.Cores.Etherbone (etherboneC)
import Clash.Cores.UART (ValidBaud)

import Protocols
import Protocols.PacketStream
import Protocols.Wishbone

{- | The buffer of the ila, with signals to control wether or not the signals get captured

This function exposes the barebones of the ILA capturing logic and it will only accept boolean
values to determine wether or not to capture samples. For more advanced trigger logic, check out
`triggerController`
-}
ilaCore ::
  forall dom size a.
  ( HiddenClockResetEnable dom
  , NFDataX a
  , BitPack a
  , KnownNat size
  , 1 <= size
  ) =>
  SNat size ->
  -- | Capture
  Signal dom Bool ->
  -- | The input signal to probe
  Signal dom a ->
  -- | Wether or not to freeze the buffer
  Signal dom Bool ->
  -- | Clear the buffer
  Signal dom Bool ->
  -- | The buffer
  (Signal dom (Index size) -> (Signal dom a, Signal dom (Index (size + 1))))
ilaCore size capture i freeze bufClear = buffer
 where
  buffer :: Signal dom (Index size) -> (Signal dom a, Signal dom (Index (size + 1)))
  buffer =
    ringBuffer
      size
      undefined
      bufClear
      (mux ((not <$> freeze) .&&. capture) (Just <$> i) (pure Nothing))

{- | Get a specific word of `a`, which may be several words wide
The width of `word` is defined by the user of the function
-}
getWord ::
  forall word input n.
  ( BitPack input
  , KnownNat word
  , KnownNat n
  , n ~ BitSize input `DivRU` word
  ) =>
  -- | The input
  input ->
  -- | The index of the word we want
  Index n ->
  -- | The output word
  BitVector word
getWord a i = (unpack . resize $ pack a :: Vec n (BitVector word)) !! i

{- | Sets a specific word of `a`, which may be several words wide
The width of the `word` is defined by the user of the function
-}
setWord ::
  forall word input n.
  ( BitPack input
  , KnownNat word
  , KnownNat n
  , n ~ BitSize input `DivRU` word
  ) =>
  -- | The input
  input ->
  -- | The index of the word we want
  Index n ->
  -- | The value we want to set it too
  BitVector word ->
  -- | The output, our input with the word swapped
  input
setWord a i w = unpack . resize . pack $ replace i w inputWords
 where
  inputWords :: Vec n (BitVector word)
  inputWords = unpack . resize $ pack a

{- | Retrieves a specific word from the ILA buffer. It will only listen to addresses within the
range of 0x3000_0000 and 0x3fff_ffff. Each address refers to a specific word (=32 bits) in the
buffer.

Samples may not precisely equal 32 bits. They may be smaller or bigger than 32 bits. However, in
this function each sample is assigned one or more addresses for each word it's width takes up.
So a 10 bit sample will only take up one address, and a 50 bit one would take up 2 addresses.
Different samples never share the same memory space.

The memory remains consecutive
-}
ilaBufferManager ::
  forall dom depth a n addrW.
  ( HiddenClockResetEnable dom
  , BitPack a
  , KnownNat depth
  , KnownNat addrW
  , KnownNat n
  , n ~ BitSize a `DivRU` 32
  , 1 <= n
  , 1 <= depth
  ) =>
  -- | The ILA buffer
  (Signal dom (Index depth) -> (Signal dom a, Signal dom (Index (depth + 1)))) ->
  -- | The incoming Wishbone M2S signal
  Signal dom (WishboneM2S addrW 4) ->
  -- | What word from the buffer entry to read
  Signal dom (Index n) ->
  -- | The specific word associated with the requested address, if the current cycle is a read cycle
  -- it will return `0` instead.
  (Signal dom (BitVector 32), Signal dom (Index (depth + 1)))
ilaBufferManager buffer incoming wordIndex = returnData
 where
  (bufferData, bufferLength) = buffer bufferIndex

  isValidAddress wb = wb.addr .&. 0xff00_0000 == 0x3200_0000
  bufferIndex = (\wb -> unpack $ resize (wb.addr .&. 0x00ff_ffff)) <$> incoming

  -- \| If in a read cycle, it will contain the output of the buffer, otherwise it will return 0
  returnData =
    ( mux
        (not . writeEnable <$> incoming .&&. isValidAddress <$> incoming)
        (liftA2 (getWord @32) bufferData wordIndex)
        (pure 0)
    , bufferLength
    )

-- | Defines how the ILA should treat the trigger select registers
data IlaPredicateOperation = PredicateAND | PredicateOR
  deriving (Generic, NFDataX, Show, Enum, BitPack)

-- | The operator used to combine the results of multiple predicates
predicateOperation :: (Foldable t) => IlaPredicateOperation -> (t Bool -> Bool)
predicateOperation PredicateAND = and
predicateOperation PredicateOR = or

{- | The default value when a predicate is not selected
This is behaviour is different depending on how they will be combined, if the end result gets
AND'd together, having the default be false would always result to the predicate system failing.
-}
predicateUnselectedDefault :: IlaPredicateOperation -> Bool -> Bool -> Bool
predicateUnselectedDefault PredicateAND False _ = True
predicateUnselectedDefault PredicateAND True result = result
predicateUnselectedDefault PredicateOR False _ = False
predicateUnselectedDefault PredicateOR True result = result

{- | A record containing the values stored in the ILA register map. Not all values are read/writeable
from the outside.

A lot of the register map is also exposed as a memory map, with the following layout:
|   Address   | Bus Select |      Description      |  Operation  |
|-------------|------------|-----------------------|-------------|
| 0x0000_0000 | 0b0001     | Capture               | Read        |
| 0x0000_0000 | 0b0010     | Trigger reset         | ReadWrite'1 |
| 0x0000_0001 | 0b1111     | Trigger point         | ReadWrite   |
| 0x0000_0002 | 0b1111     | ILA hash              | Read        |
| 0x0000_0003 | 0b0001     | ILA trigger operation | ReadWrite   |
| 0x0000_0004 | 0b1111     | ILA trigger select    | ReadWrite'2 |
| 0x0000_0005 | 0b0001     | ILA capture operation | ReadWrite   |
| 0x0000_0006 | 0b1111     | ILA capture select    | ReadWrite'2 |
| 0x0000_0007 | 0b1111     | Sample count          | Read        |
| 0x1000_0000 | 0b1111     | ILA trigger mask      | ReadWrite   |
| 0x1100_0000 | 0b1111     | ILA trigger compare   | ReadWrite   |
| 0x2000_0000 | 0b1111     | ILA capture mask      | ReadWrite   |
| 0x2100_0000 | 0b1111     | ILA capture compare   | ReadWrite   |
| 0x3100_0000 | 0b1111     | Word index            | Write'3     |
| 0x3200_0000 | 0b1111     | Perform read          | Read'4      |

'1: Reading from this address will return wether or not the ILA has been triggered or not
'2: Each bit is for one predicate
'3: Due to the etherbone requiring packets be split in packets of 32 bits, reading the memory
    requires the end user to provide two indices. The specific buffer index the user would like to
    read and which word of that buffer entry.
'4: Actually performs the memory reads from the internal ILA buffer, the buffer index is the
    address being accessed as long as it is within 0x3200_0000 0x32ff_ffff.

Future optimisation: introduce `Buffer length` register to perform multiple reads automatically
without requiring manual reads. This will require a somewhat large overhaul of the TUI.
-}
data IlaRM bitSizeA depth n = IlaRM
  { capture :: Bool
  -- ^ If the ILA should be capturing signals
  , triggered :: Bool
  -- depending on the `triggerPoint` it may still be sampling new values.
  , triggerPoint :: Index depth
  -- ^ The amount of signals to capture *after* triggering
  , shouldSample :: Bool
  -- ^ Indicates if the ILA should continue sampling data. This gets automatically set after the
  -- ILA has been triggered and enough samples have been collected. It will only get out of this
  -- state by resetting the ILA.
  , sampledAfterTrigger :: Index depth
  -- ^ The amount of samples the ILA captured after triggering
  , sampleCount :: Index (depth + 1)
  -- ^ The amount of samples currently being stored in the buffer
  , triggerSelect :: BitVector 32
  -- ^ Which trigger predicate to use
  , triggerOperation :: IlaPredicateOperation
  -- ^ How the ILA treats the trigger predicates in combination with `triggerSelect` bits
  , triggerMask :: BitVector bitSizeA
  -- ^ The mask given to the trigger predicate
  , triggerCompare :: BitVector bitSizeA
  -- ^ The compare value given to the trigger predicate
  , captureSelect :: BitVector 32
  -- ^ Which capture predicate to use
  , captureOperation :: IlaPredicateOperation
  -- ^ How the ILA treats the capture predicates in combination with `captureSelect` bits
  , captureMask :: BitVector bitSizeA
  -- ^ The mask given to the capture predicate
  , captureCompare :: BitVector bitSizeA
  -- ^ The compare value given to the capture predicate
  , wordIndex :: Index (bitSizeA `DivRU` 32)
  -- ^ What buffer to read
  }
  deriving (Generic, NFDataX, Show)

{- | Read from memory mapped registers in the register map using the addresses selected from a
wishbone packet
-}
readIlaMM ::
  forall a depth n.
  ( KnownNat depth
  , KnownNat a
  , KnownNat n
  , 1 <= n
  , 1 <= depth
  , 1 <= a `DivRU` 32
  , 1 <= a
  ) =>
  -- | The wishbone address
  BitVector 32 ->
  -- | The wishbone bus select
  BitVector 4 ->
  -- | The ILA MM
  IlaRM a depth n ->
  -- | Associated value with the address and bus select, `Nothing` if there is none
  Maybe (BitVector 32)
readIlaMM 0x0000_0000 0b0001 rm = Just . extend $ pack rm.capture
readIlaMM 0x0000_0000 0b0010 rm = Just . extend . pack $ not rm.shouldSample
readIlaMM 0x0000_0001 0b1111 rm = Just . resize $ pack rm.triggerPoint
readIlaMM 0x0000_0003 0b0001 rm = Just . resize $ pack rm.triggerOperation
readIlaMM 0x0000_0004 0b1111 rm = Just rm.triggerSelect
readIlaMM 0x0000_0005 0b0001 rm = Just . resize $ pack rm.captureOperation
readIlaMM 0x0000_0006 0b1111 rm = Just rm.captureSelect
readIlaMM 0x0000_0007 0b1111 rm = Just . resize $ pack rm.sampleCount
readIlaMM address 0b1111 rm
  | testBits address 0x1000_0000 0xff00_0000 =
      Just $ getWord rm.triggerMask index
  | testBits address 0x1100_0000 0xff00_0000 =
      Just $ getWord rm.triggerCompare index
  | testBits address 0x2000_0000 0xff00_0000 =
      Just $ getWord rm.captureMask index
  | testBits address 0x2100_0000 0xff00_0000 =
      Just $ getWord rm.captureCompare index
 where
  index = unpack . resize $ address .&. 0x00ff_ffff
readIlaMM _ _ _ = Nothing

-- | Tests if multiple bits (via a mask) match
testBits ::
  (Bits a) =>
  -- | Value to test
  a ->
  -- | The compare value
  a ->
  -- | The bit mask
  a ->
  -- | True is the bits match
  Bool
testBits value ref msk = (value .&. msk) == ref

{- | Certain registers trigger 'actions' rather than save a value, this enum allows the ILA to
properly respond to those requests
-}
data IlaAction = None | ResetTrigger
  deriving (Generic, Eq, NFDataX, Show, Enum)

-- | Update the memory mapped registers from the register map from the wishbone request
writeIlaMM ::
  forall a depth n.
  ( KnownNat depth
  , KnownNat a
  , KnownNat n
  , 1 <= depth
  , 1 <= n
  , 1 <= a `DivRU` 32
  , 1 <= a
  ) =>
  -- | The wishbone address
  BitVector 32 ->
  -- | The wishbone bus select
  BitVector 4 ->
  -- | The value to write
  BitVector 32 ->
  -- | The Ila register map
  IlaRM a depth n ->
  -- | Updated Ila register map, invalid addresses will not modify the register map
  (IlaRM a depth n, IlaAction)
writeIlaMM 0x0000_0000 0b0001 write rm = (rm{capture = unpack $ truncateB write}, None)
writeIlaMM 0x0000_0000 0b0010 _write rm = (rm{triggered = False}, ResetTrigger)
writeIlaMM 0x0000_0001 0b1111 write rm = (rm{triggerPoint = unpack $ resize write}, None)
writeIlaMM 0x0000_0003 0b0001 write rm = (rm{triggerOperation = unpack $ resize write}, None)
writeIlaMM 0x0000_0004 0b1111 write rm = (rm{triggerSelect = write}, None)
writeIlaMM 0x0000_0005 0b0001 write rm = (rm{captureOperation = unpack $ resize write}, None)
writeIlaMM 0x0000_0006 0b1111 write rm = (rm{captureSelect = write}, None)
writeIlaMM address 0b1111 write rm
  | testBits address 0x1000_0000 0xff00_0000 =
      (rm{triggerMask = setWord rm.triggerMask index write}, None)
  | testBits address 0x1100_0000 0xff00_0000 =
      (rm{triggerCompare = setWord rm.triggerCompare index write}, None)
  | testBits address 0x2000_0000 0xff00_0000 =
      (rm{captureMask = setWord rm.captureMask index write}, None)
  | testBits address 0x2100_0000 0xff00_0000 =
      (rm{captureCompare = setWord rm.captureCompare index write}, None)

  | testBits address 0x3100_0000 0xff00_0000 =
      (rm{wordIndex = unpack $ resize write}, None)
 where
  -- We use 32 bit words, the indices incrementing writes receive is on a byte basis
  -- To correct for this, we can simply shift right by 2 (dividing by 4)
  wordCorrection = 2
  index = unpack . resize $ (address .&. 0x00ff_ffff) .>>. wordCorrection
writeIlaMM _ _ _ rm = (rm, None)

{- | ILA Wishbone interface

This circuit can be used to instantiate and configure an ILA using a wishbone interface. Changing
ILA behaviour is done by writing to specific addresses and selecting the right bytes using busSelect.
-}
ilaWb ::
  forall dom.
  (HiddenClockResetEnable dom) =>
  -- | Initial ILA configuration
  IlaConfig dom ->
  -- | The ILA wishbone interface
  Circuit
    (Wishbone dom Standard 32 4)
    ()
ilaWb (IlaConfig @_ @a @depth @m depth initTriggerPoint ilaHash tracing predicates) = Circuit exposeIn
 where
  exposeIn (fwdM2S, _) = out
   where
    -- \| The initial contents of the memory map
    initRM :: IlaRM (BitSize a) depth m
    initRM =
      IlaRM
        { capture = False
        , triggered = False
        , triggerPoint = initTriggerPoint
        , shouldSample = False
        , sampledAfterTrigger = 0
        , sampleCount = 0
        , triggerOperation = PredicateOR
        , triggerSelect = minBound
        , triggerMask = maxBound
        , triggerCompare = 0
        , captureOperation = PredicateOR
        , captureSelect = minBound
        , captureMask = maxBound
        , captureCompare = 0
        , wordIndex = 0
        }

    -- \| Update parts of the RM which aren't depending on input from WB
    updateRM ::
      -- \| If the predicate got triggered
      Bool ->
      -- \| If the capture is set
      Bool ->
      -- \| The amount of samples currently being stored in the buffer
      Index (depth + 1) ->
      -- \| The current register map
      IlaRM (BitSize a) depth m ->
      -- \| The updated register map
      IlaRM (BitSize a) depth m
    updateRM triggered capture buffLength rm =
      rm
        { triggered = rm.triggered || triggered
        , capture = capture
        , sampledAfterTrigger = if rm.triggered then satAdd SatBound 1 rm.sampledAfterTrigger else 0
        , shouldSample = not rm.triggered || rm.sampledAfterTrigger < rm.triggerPoint
        , sampleCount = buffLength
        }

    -- \| Selects the right predicate and applies it on incoming sample
    doesTrigger :: IlaRM (BitSize a) depth m -> a -> Vec m (RawPredicate a) -> Bool
    doesTrigger rm currentSample predicates' =
      rm.capture -- If capture is disabled, don't bother with testing predicates
        && ( predicateOperation rm.triggerOperation $
              zipWith
                (predicateUnselectedDefault rm.triggerOperation)
                (reverse . unpack $ resize rm.triggerSelect)
                ((\predicate -> predicate currentSample rm.triggerCompare rm.triggerMask) <$> predicates')
           )

    -- \| Selects the right predicate and applies it on incoming sample
    captureActive :: IlaRM (BitSize a) depth m -> a -> Vec m (RawPredicate a) -> Bool
    captureActive rm currentSample predicates' =
      predicateOperation rm.captureOperation $
        zipWith
          (predicateUnselectedDefault rm.captureOperation)
          (reverse . unpack $ resize rm.captureSelect)
          ((\predicate -> predicate currentSample rm.captureCompare rm.captureMask) <$> predicates')

    -- \| The transfer function of the moore state machine
    --
    -- This function will update the register map of the ILA, always first attempting to update
    -- the ILA with wishbone write requests, then updating non-wishbone dependent aspects of the ILA
    transfer (oldRM, _) (wb, currentSample, buffLength, predicates') = wbUpdate regularUpdate
     where
      wbUpdate
        | wb.strobe && wb.busCycle && wb.writeEnable = writeIlaMM wb.addr wb.busSelect wb.writeData
        -- If there's no wishbone write request, simply id
        | otherwise = \rm -> (rm, None)

      regularUpdate =
        updateRM
          (doesTrigger oldRM currentSample predicates')
          (captureActive oldRM currentSample predicates')
          buffLength
          oldRM

    -- \| Handle WB writes and update the memory map accordingly,
    (ilaRM, ilaAction) =
      unbundle $
        moore
          transfer
          id
          (initRM, None)
          (bundle (fwdM2S, tracing, bufferLength, bundle predicates))

    -- \| If the trigger gets triggered in the current sample, the rest of the system will only know
    -- that the next cycle. This means that there will be an off-by-one error for capturing the
    -- samples
    --
    -- To combat this, we simply delay the signal we sample by one cycle!
    delayedTrace = register (unpack 0) tracing

    -- \| The output from the ILA's internal buffer
    (bufferOutput, bufferLength) =
      ilaBufferManager
        ( ilaCore
            depth
            ilaRM.capture
            delayedTrace
            (not <$> ilaRM.shouldSample)
            ((== ResetTrigger) <$> ilaAction)
        )
        fwdM2S
        ilaRM.wordIndex

    -- \| Distribute the read request to different parts of the ILA depending on the address & bus select
    -- It will first attempt to read from the register map (& constant values), if there's no valid
    -- address there. It will return the buffer content. If an invalid address gets read there,
    -- it will return zero.
    readManager' ::
      -- \| The current WB packet
      WishboneM2S 32 4 ->
      -- \| The ila register map
      IlaRM (BitSize a) depth m ->
      -- \| The value the buffer is currently pointing at
      BitVector 32 ->
      -- \| The value the component should respond with
      BitVector 32
    readManager' wb rm bufValue
      | wb.addr == 0x0000_0002 && wb.busSelect == 0b1111 = ilaHash
      | otherwise =
          case readIlaMM wb.addr wb.busSelect rm of
            Just v -> v
            Nothing
              | rm.shouldSample -> 0
              | otherwise -> bufValue
    readManager =
      mux
        ((\fwd -> fwd.busCycle && fwd.strobe && (not fwd.writeEnable)) <$> fwdM2S)
        (liftA3 readManager' fwdM2S ilaRM bufferOutput)
        (pure maxBound)

    -- \| Generates the wishbone reply
    reply ::
      -- \| Wether or not we're in a cycle
      Bool ->
      -- \| The data to reply with (if in a read cycle)
      BitVector 32 ->
      -- \| The wishbone response
      WishboneS2M 4
    reply inCyc dat =
      WishboneS2M
        { readData = dat
        , acknowledge = inCyc
        , err = False
        , stall = False
        , retry = False
        }

    -- \| The first clock cycle shouldn't ack according to wishbone
    delayedAck = inWbCycle <$> fwdM2S .&&. (register False $ inWbCycle <$> fwdM2S)
     where
      inWbCycle fwd = fwd.strobe && fwd.busCycle

    -- Writes are done in one clock cycle, but wishbone timing requires us to delay it by one clock cycle
    out =
      ( register emptyWishboneS2M $ liftA2 reply delayedAck readManager
      , ()
      )

{- | The ILA component itself

Given any set of signals and a trigger condition, it will be capable of sampling the signals up until
it is triggered. At which point the connected host device can send commands to retrieve / re-arm the
trigger. Data communication is being done through `Etherbone` packets which get sent from the host
device. If run configuration of the ILA on the FPGA is desired, please look at `ilaWb` to instantiate
an ILA with an Wishbone interface instead.
-}
ila ::
  forall dom.
  (HiddenClockResetEnable dom) =>
  -- | The initial configuration of the ILA
  IlaConfig dom ->
  -- | The ILA circuit, the incoming packet stream should contain valid `Etherbone` packets. The
  -- outgoing stream are etherbone response packets.
  Circuit
    (PacketStream dom 4 ())
    (PacketStream dom 4 ())
ila config = circuit $ \incoming -> do
  (outgoing, wbMaster) <- etherboneC 0 (pure Nil) -< incoming

  ilaWb config -< wbMaster

  idC -< outgoing

{- | UART wrapper around the ILA. A simple drop-in replacement for `ila`. Useful if the only
connection to the host PC is an UART connection.
-}
ilaUart ::
  forall dom baud.
  ( HiddenClockResetEnable dom
  , ValidBaud dom baud
  ) =>
  SNat baud ->
  -- | The initial configuration of the ILA
  IlaConfig dom ->
  -- | The ILA circuit but with signals of bits as input and output. These should be directly wired
  -- to the toplevel UART RX and TX pins.
  Circuit
    (CSignal dom Bit)
    (CSignal dom Bit)
ilaUart baud config = circuit $ \rxBit -> do
  (rxByte, txBit) <- uartDf baud -< (txByte, rxBit)

  rxPs <- etherboneDfPacketizer <| holdUntilAck -< rxByte
  txByte <- ps2df -< txPs

  txPs <- ila config -< rxPs

  idC -< txBit
