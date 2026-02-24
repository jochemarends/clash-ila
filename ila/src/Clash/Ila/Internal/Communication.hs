{-# LANGUAGE DuplicateRecordFields #-}
{-# LANGUAGE OverloadedRecordDot #-}
{-# LANGUAGE NoFieldSelectors #-}

module Clash.Ila.Internal.Communication where

import Clash.Cores.UART (ValidBaud, uart)
import Clash.Prelude

import Protocols
import Protocols.Df qualified as Df
import Protocols.PacketStream

import Data.Maybe qualified as DM

{- | Send out `Df` data through UART

In essence just a tiny wrapper around `uart` to provide backpressure using `Circuit`s rather than
manual signals
-}
uartDf ::
  forall dom baud.
  (HiddenClockResetEnable dom, ValidBaud dom baud) =>
  -- | Uart BAUD rate
  SNat baud ->
  -- | The UART circuit, a small wrapper around default uart to make it use Df instead of raw signals
  -- The inputs are: transmission byte, rx bit
  -- The outputs are: recieved byte, tx bit
  Circuit
    (Df dom (BitVector 8), CSignal dom Bit)
    (CSignal dom (Maybe (BitVector 8)), CSignal dom Bit)
uartDf baud = Circuit exposeIn
 where
  exposeIn ((transmit, rxBit), _) = out
   where
    (recieved, txBit, acked) = uart baud rxBit transmit

    out =
      ( (Ack <$> acked, ())
      , (recieved, txBit)
      )

-- | Holds a certain value until we get an `Ack`, only then will it accept new values
holdUntilAck ::
  forall dom.
  (HiddenClockResetEnable dom) =>
  -- | The data you want to hold
  -- It only accepts new data if it is currently not storing any, OR we got a positive acknowledgement
  -- The output will be the data currently held, if any
  Circuit
    (CSignal dom (Maybe (BitVector 8)))
    (Df dom (BitVector 8))
holdUntilAck = Circuit exposeIn
 where
  exposeIn (incoming, ack) = out
   where
    hold :: (Signal dom (Maybe (BitVector 8)))
    hold =
      register Nothing $
        mux
          (ackToBool <$> ack .||. DM.isNothing <$> hold)
          incoming
          hold

    -- I'm honestly surprised that this isn't a thing already
    -- There are so many nice Df utility functions, yet this one misses
    -- Oh well
    ackToBool :: Ack -> Bool
    ackToBool (Ack b) = b

    out = ((), hold)

{- | Silently drops the meta of a `PacketStream`
Makes no other modifications to the `PacketStream` itself
-}
dropMeta ::
  forall dom a meta.
  Circuit
    (PacketStream dom a meta)
    (PacketStream dom a ())
dropMeta = mapMeta (\_ -> ())

{- | Converts `PacketStream` into a `Df` of bytes. For packet stream data larger than one byte,
it will send out bytes in most-significant-byte order.

NOTE: Please do properly follow the PacketStream standard and keep sending the same data until
`_ready` is raised. Not doing so will cause issues.
-}
ps2df ::
  forall dom dataWidth.
  ( HiddenClockResetEnable dom
  , KnownNat dataWidth
  , 1 <= dataWidth
  ) =>
  -- | The input `PacketStream` and the output `Df`, will only set `_ready` to true once every
  -- byte in PacketStream has been sent over in Df
  Circuit
    (PacketStream dom dataWidth ())
    (Df dom (BitVector 8))
ps2df = Circuit exposeIn <| downConverterC @1
 where
  exposeIn (incoming, backpressure) = out
   where
    toDf :: Maybe (PacketStreamM2S 1 ()) -> Maybe (BitVector 8)
    toDf (Just packet) = Just $ at d0 (_data packet)
    toDf Nothing = Nothing

    ack2ps :: Ack -> PacketStreamS2M
    ack2ps (Ack b) = PacketStreamS2M b

    out =
      ( ack2ps <$> backpressure
      , toDf <$> incoming
      )

-- | Represents state of `etherboneDfPacketizer`
data DepacketizeDfState
  = -- | Currently not parsing / searching for the magic
    EBMagic
  | -- | Parsing the EBHeader
    EBHeader (Index 1)
  | -- | EBPadding (Index 1)
    EBRecord
      -- | Parse the header of an EBRecord, this is the field containing how many read/writes there are
      (Index 2)
  | -- | The value for wCound and rCount are 8 bit, meaning there are at most 256 entries (for each).
    -- There's also the base address fields, so that makes the total 256 * 2 + 2 = 514
    --
    -- The second index is for 32 and 64 bit mode, where the entry may take up multiple 16 bit words
    -- In our case, we only support 32 bit etherbone, so it's always `Index 2`
    EBBody (Index 514, Index 2)
  deriving (Generic, NFDataX, Show)

{- | On a Df input, packetize the data such that it can be fed into `etherboneC` without trouble.

The problem this function intends to solve is that for certain kinds of mediums, the complete
packet length isn't known ahead of time. Making the construction of the `PacketStream` difficult.

This function allows one to have a stream of bytes and it will attempt to format it into a
`PacketStream`, so it can be fed to `etherboneC`.

# Limitations

For now, this function __ONLY__ supports 32 bit etherbone packets. 64 bit etherbone packets will
get treated as 32 bit, which __WILL__ cause you problems if you try it. So don't :)

Another limitation is that due to the nature of etherbone, it is impossible for us to determine
how many EBRecords there are. This function assumes each etherbone packet only contains
__one__ EBRecord.
-}
etherboneDfPacketizer ::
  forall dom.
  (HiddenClockResetEnable dom) =>
  Circuit
    (Df dom (BitVector 8))
    (PacketStream dom 4 ())
etherboneDfPacketizer = upConverterC @2 @2 <| Circuit exposeIn <| Df.compressor Nothing toWord <| Df.fifo d32
 where
  toWord ::
    (Maybe (BitVector 8)) -> BitVector 8 -> (Maybe (BitVector 8), Maybe (Vec 2 (BitVector 8)))
  toWord Nothing m = (Just m, Nothing)
  toWord (Just m) l = (Nothing, Just $ m :> l :> Nil)

  exposeIn (fwd, bwd) = out
   where
    -- \| Parse WCount and RCount fields in an EBRecord and return how long the expected packet is
    --
    -- We *2 is done due to us assuming we're using 32 bit etherbone, so each address/value is
    -- 32 bits. Each field is 16 bits.
    getLength :: Vec 2 (BitVector 8) -> Index 514
    getLength word = case word of
      (0 :> 0 :> Nil) -> 0
      (wCount :> 0 :> Nil) -> unpack $ resize wCount
      (0 :> rCount :> Nil) -> unpack $ resize rCount
      (wCount :> rCount :> Nil) -> (unpack $ resize rCount) + (unpack $ resize wCount) + 1
      _ -> 0 -- Haskell is stupid

    -- \| Create the PS packet without `_last`
    partialPs word =
      Just $
        PacketStreamM2S
          { _last = Nothing
          , _abort = False
          , _meta = ()
          , _data = word
          }
    -- \| Create the PS packet with `_last`
    completePS word =
      Just $
        PacketStreamM2S
          { _last = Just maxBound
          , _abort = False
          , _meta = ()
          , _data = word
          }

    transfer' ::
      DepacketizeDfState ->
      Maybe (Vec 2 (BitVector 8)) ->
      (DepacketizeDfState, Maybe (PacketStreamM2S 2 ()))
    transfer' EBMagic (Just word@(0x4e :> 0x6f :> Nil)) = (EBHeader maxBound, partialPs word)
    transfer' EBMagic _ = (EBMagic, Nothing) -- Silently drop packets if they're not part of etherbone
    transfer' (EBHeader n) (Just word)
      | n == 0 = (EBRecord maxBound, partialPs word)
      | otherwise = (EBHeader (n - 1), partialPs word)
    transfer' (EBRecord n) (Just word)
      | n == 0 && getLength word == 0 = (EBMagic, completePS word)
      | n == 0 = (EBBody (getLength word, maxBound), partialPs word)
      | otherwise = (EBRecord (n - 1), partialPs word)
    transfer' (EBBody (n, w)) (Just word)
      | n == 0 && w == 0 = (EBMagic, completePS word)
      | w == 0 = (EBBody (n - 1, maxBound), partialPs word)
      | otherwise = (EBBody (n, w - 1), partialPs word)
    -- When we receive no data, keep the current state
    transfer' state Nothing = (state, Nothing)

    transfer ::
      DepacketizeDfState ->
      (PacketStreamS2M, Maybe (Vec 2 (BitVector 8))) ->
      (DepacketizeDfState, Maybe (PacketStreamM2S 2 ()))
    transfer state (bwdData, fwdData)
      -- \| _ready bwdData = trace [__i|#{state} with #{fwdData} -> #{transfer' state fwdData}|] $ transfer' state fwdData
      | _ready bwdData = transfer' state fwdData
      | otherwise = (state, Nothing)

    outwardsData :: Signal dom (Maybe (PacketStreamM2S 2 ()))
    outwardsData = mealy transfer EBMagic (bundle (bwd, fwd))

    out = (Ack . DM.isJust <$> outwardsData, outwardsData)

{- | Given a stream of bytes, replace the `_last` with `Nothing`, every `n`th packet will have
`Just 1` set. This is useful in combination with `upConverterC`
-}
mergeBytePackets ::
  forall dom n a.
  ( HiddenClockResetEnable dom
  , KnownNat n
  , 1 <= n
  ) =>
  -- | How many packets need to be bundled together
  SNat n ->
  Circuit
    (PacketStream dom 1 a)
    (PacketStream dom 1 a)
mergeBytePackets _ = Circuit exposeIn
 where
  exposeIn (fwdIn, bwdIn) = out
   where
    transfer ::
      Index n -> Maybe (PacketStreamM2S 1 a) -> (Index n, Maybe (PacketStreamM2S 1 a))
    transfer _ Nothing = (0, Nothing)
    transfer i (Just packet)
      | i == natToNum @(n - 1) = (0, Just packet{_last = Just 1})
      | otherwise = (i + 1, Just packet{_last = Nothing})

    out = (bwdIn, mealy transfer 0 fwdIn)
